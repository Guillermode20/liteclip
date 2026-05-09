# Performance Analysis: Output / MP4 Muxing & Clip Export Pipeline

**Date:** 2026-05-09
**Scope:** `crates/liteclip-core/src/output/` — MP4 muxing, clip saver, SDK export, video file writing, companion cache.

---

## Summary

The output pipeline spans three major workflows:

1. **Live clip saving** (`saver.rs` → `types.rs` → `mp4.rs`) — snapshot packets from the ring buffer, mux into MP4, generate thumbnail.
2. **SDK clip export** (`video_file.rs` → `sdk_export.rs`) — decode existing MP4, apply trim/crop/resize/post-process, re-encode with bitrate calibration.
3. **Stream copy export** (`sdk_export.rs` → `sdk_ffmpeg_output.rs`) — fast trim/concat without re-encoding.

All three run on **blocking threads** (`tokio::task::spawn_blocking` or `thread::spawn`), so no tokio runtime blocking. The codebase already has **aggressive memory management** (explicit `drop`, `shrink_to_fit`, hint allocations, `log_save_memory`). However, there are several areas where performance can be improved: multi-pass calibration overhead, per-range resource recreation during export, allocation churn in audio mixing, and unbounded intermediate buffers.

---

## Hot Paths

| Rank | Path | Workflow | Typical Time |
|------|------|----------|------|
| 1 | `attempt_export()` frame decode → scale → post-process → encode → mux | Export | 1–5× realtime |
| 2 | `calibrate_initial_bitrate()` → 2× `attempt_export` on samples | Export calibration | 20–40% of total export time |
| 3 | `mix_and_encode_audio_chunks()` → PCM mixing → resample → encode | Clip save | 5–15% of mux time |
| 4 | `write_packets()` interleaved muxing loop | Clip save | 10–20% of clip save |
| 5 | `normalize_video_packets_for_mp4()` → merge SPS/PPS into frames | Clip save | 1–5% (only when needed) |

---

## Findings

### F1 — Calibration run generates 2 full decode-encode cycles (P1)

| Field | Value |
|-------|-------|
| **Location** | `video_file.rs:399` — `calibrate_initial_bitrate()` |
| **Severity** | High |
| **Description** | Before the actual export, calibration runs two full decode-encode-mux passes on 4–18 second sample segments. For short clips (<30s), this doubles or triples total export time. |
| **Code** | ```rust
fn calibrate_initial_bitrate(...) -> Result<Option<u32>> {
    // ...
    let low_result = attempt_export(&sample_request, &cal_low_path, low_bitrate_kbps, ...)?;
    let high_result = attempt_export(&sample_request, &cal_high_path, high_bitrate_kbps, ...)?;
    // ...
}
``` |
| **Why** | Calibration is required to hit the target file size accurately (within 90–100% of target MB). Without it, the power-law bitrate estimate is unreliable across different content types. |
| **Recommendation** | (a) Cache calibration results keyed by `(input_path, encoder, (width,height,fps), output_complexity_ratio)` so re-exports of the same clip skip calibration. (b) Consider a single-point calibration (one sample at the estimated bitrate) with a correction factor instead of two full samples. (c) For clips under 15s, skip calibration and use the conservative estimate directly — the size error is bounded by the small duration. |

---

### F2 — Decoder, scaler, and filter graph recreated per keep range (P2)

| Field | Value |
|-------|-------|
| **Location** | `sdk_export.rs:775–820` — per-range setup inside `attempt_export()` |
| **Severity** | Medium |
| **Description** | For each trim range in a multi-segment export, the video decoder, audio decoder, scaler, audio resampler, and post-process filter graph are all destroyed and recreated from scratch. This involves FFmpeg internal memory pool allocations. |
| **Code** | ```rust
for range in &request.keep_ranges {
    seek_to_seconds(&mut input_ctx, range.start_secs);
    // Recreate decoders after each seek
    let v_stream = input_ctx.stream(video_stream_idx)...;
    let v_ctx = Context::from_parameters(v_stream.parameters())?;
    let mut video_decoder = v_ctx.decoder().video()?;
    let mut video_scaler = scaling::Context::get(...)?;
    let mut post_process_graph: Option<PostProcessFilterGraph> = None;
    // ... fresh allocation
}
``` |
| **Why** | FFmpeg does not support seeking + resuming decoding without flushing/recreating the decoder. The scaler and filter graph depend on video dimensions that are identical across ranges, so they are wasted allocations. |
| **Recommendation** | (a) Move the **scaler** and **post-process filter graph** outside the range loop — they don't depend on per-range state (same resolution, same encoder format). Only the **decoder** must be recreated after each seek. (b) The audio resampler can also be reused across ranges since sample format/rate is constant. |

---

### F3 — Audio mixing allocates intermediate i32 buffer per 1-second chunk (P3)

| Field | Value |
|-------|-------|
| **Location** | `mp4.rs` — `mix_and_encode_audio_chunks()` |
| **Severity** | Medium |
| **Description** | Audio mixing processes in 1-second chunks, allocating a `mixed_i32[chunk_samples]` (192,000 ints = 768 KB) and `mixed_i16[chunk_samples]` (384 KB) per chunk. The i32→i16 conversion with custom soft-clipping uses elementwise f32 math with no SIMD. |
| **Code** | ```rust
let chunk_samples = AUDIO_SAMPLE_RATE as usize * AUDIO_CHANNELS as usize; // 96k
let mut mixed_i32 = vec![0_i32; chunk_samples]; // 768KB
let mut mixed_i16 = vec![0_i16; chunk_samples]; // 384KB
// ...
for (i, &sample) in mixed_i32[..current_chunk_len].iter().enumerate() {
    let limit = 24000.0;
    let sample_f32 = sample as f32;
    let clipped = if sample_f32 > limit { /* complex soft clip */ }
    // ...
    mixed_i16[i] = clipped.clamp(-32768.0, 32767.0).round() as i16;
}
``` |
| **Why** | Soft-clipping is needed to prevent audio artifacts when two audio streams (system + mic) are summed. Using f32 per-sample is correct but slow without SIMD. |
| **Recommendation** | (a) Reuse `mixed_i32` and `mixed_i16` buffers across chunks instead of re-allocating each time. Move the `vec![]` allocations outside the while loop and `fill(0)` the active range. (b) Replace elementwise f32 soft-clipping with a SIMD implementation using `core_simd` or `wide` crate when available. (c) Consider an i16-safe addition with bit-level saturation instead of f32 soft-clipping for a 3–5× speedup (soft-clip only for the ~0.1% of samples that actually clip). |

---

### F4 — Encoded audio packet vec is unbounded until flush (P3)

| Field | Value |
|-------|-------|
| **Location** | `mp4.rs` — `mix_and_encode_audio_chunks()` return value and `write_packets()` usage |
| **Severity** | Medium |
| **Description** | `mix_and_encode_audio_chunks()` returns `Vec<EncodedAudioPacket>` containing ALL encoded AAC packets for the entire clip. For a 30-minute recording at ~10 KB/packet (1024 samples), this can be ~84,000 packets → ~840 MB of audio data held in memory before any packets are written to the muxer. |
| **Code** | ```rust
let encoded_audio: Vec<EncodedAudioPacket> = if let (Some(_), Some(audio_encoder)) = ... {
    mix_and_encode_audio_chunks(audio_encoder, audio_packets, base_qpc, video_end_qpc)?
};
// ... then later iterated with aac_iter
``` |
| **Why** | The audio must be fully mixed, resampled, and encoded before the interleaved write loop can interleave video and audio by DTS. The encoding happens as a blocking step before any muxer output. |
| **Recommendation** | (a) Change the architecture to write audio packets to the muxer as they are encoded, rather than batching all AAC packets. This requires restructuring `mix_and_encode_audio_chunks` to accept a callback/writer. (b) Even simpler: flush audio packets to a temp file and stream them back, reducing peak memory from O(clip_duration) to O(1 second). |

---

### F5 — Runtime video codec detection scans all packets (P3)

| Field | Value |
|-------|-------|
| **Location** | `types.rs` — `Muxer::detect_video_codec()` |
| **Severity** | Low |
| **Description** | Codec detection iterates through all video packets to detect H.264 vs HEVC parameter sets. For a 60-minute clip at 60 FPS, this scans 216,000 packets just to determine the codec. The actual codec is known at encode time but is not passed through. |
| **Code** | ```rust
fn detect_video_codec(video_packets: &[&EncodedPacket], fallback: &str) -> String {
    for packet in video_packets {
        if matches!(h264_nal_type(data), Some(7 | 8)) { return "h264".to_string(); }
        if matches!(hevc_nal_type(data), Some(32..=34)) { return "hevc".to_string(); }
    }
    fallback.to_string()
}
``` |
| **Why** | The encoder type is known when packets are generated (it's stored in `EncoderConfig`), but the codec type is not propagated through `EncodedPacket` or the ring buffer. Detection exists because the capture pipeline might use a different codec than configured. |
| **Recommendation** | (a) Add a `codec` field to `EncodedPacket` or `StreamType::Video` variant so detection is O(1). (b) Alternatively, pass the configured codec through `MuxerConfig` and only run detection as a verification step on the first few packets (break early after first VCL NAL is found). |

---

### F6 — Memory hint drops before/after calibration are fragile (P2)

| Field | Value |
|-------|-------|
| **Location** | `video_file.rs` — `calibrate_initial_bitrate()` and `run_bitrate_search()` |
| **Severity | Low |
| **Description** | Between calibration/attempt runs, the code drops an empty `Vec<u8>` to "encourage release of FFmpeg's internal memory pools". This is a best-effort hint that may have no effect on the allocator, and its effectiveness is platform/allocator-dependent. |
| **Code** | ```rust
// Hint to the OS that we've finished a major allocation phase
std::mem::drop(std::vec::Vec::<u8>::with_capacity(0));
``` |
| **Why** | `Vec::with_capacity(0)` does not allocate, so `drop` on it does nothing meaningful. The comment suggests intent but the implementation is a no-op. |
| **Recommendation** | (a) Use `std::alloc::dealloc` on a properly allocated 1-byte block to signal to jemalloc/mimalloc, or (b) call `ffmpeg::ffi::av_fast_malloc` with a zero'd pointer to hint pool release. (c) If the goal is just to force a new page allocation, allocate a 4KB page and drop it. (d) Remove the hint and rely on explicit `drop()` of heavyweight FFmpeg objects. |

---

### F7 — Normalize video packets creates extra Vec copy even when no merging needed (P2)

| Field | Value |
|-------|-------|
| **Location** | `types.rs` — `Muxer::mux_clip()` |
| **Severity** | Medium |
| **Description** | When normalization IS needed, `normalize_video_packets_for_mp4` creates a full new `Vec<EncodedPacket>` with cloned data for merged packets. When normalization is NOT needed, the `raw_video_packets` Vec of references is reused directly. However, the audio packet vector is always cloned via `.iter().filter().collect()`. |
| **Code** | ```rust
let mut raw_video_packets: Vec<&EncodedPacket> = packets.iter()
    .filter(|packet| matches!(packet.stream, StreamType::Video)).collect();
let mut audio_packets: Vec<&EncodedPacket> = packets.iter()
    .filter(|packet| matches!(packet.stream, SystemAudio | Microphone)).collect();
// ...
if needs_normalization {
    normalized_storage = normalize_video_packets_for_mp4(&raw_video_packets);
    video_refs = normalized_storage.iter().collect();
    drop(raw_video_packets);
} else {
    video_refs = raw_video_packets;
}
``` |
| **Why** | The codec (NVENC/AMF) may emit standalone SPS/PPS packets that need to be merged with the following VCL packet for the MP4 muxer. Without the check, every clip would pay the allocation cost. |
| **Recommendation** | (a) The early-return path is already optimal (reuses refs). (b) Consider a single-pass partition: `packets.iter().partition()` to avoid two filter+collect passes on the full packet list. (c) Use `Iterator::filter_map` with a small enum to split video/audio in one pass. |

---

### F8 — Stream copy re-encodes timestamps per packet (P2)

| Field | Value |
|-------|-------|
| **Location** | `sdk_export.rs:570–620` — stream copy packet loop |
| **Severity** | Low |
| **Description** | In the stream copy path, every packet goes through `rescale_ts`, then manual PTS/DTS adjustment with per-stream monotonicity enforcement. For very long stream-copy exports (hours of footage), this scales linearly. |
| **Code** | ```rust
packet.rescale_ts(route.in_time_base, route.out_time_base);
// then manual fixed_pts/fixed_dts adjustment
if let Some(last) = last_out_dts_by_stream.get(&route.out_idx) {
    if fixed_dts < *last { fixed_dts = *last; }
}
``` |
| **Why** | FFmpeg's `rescale_ts` can produce non-monotonic timestamps near time base boundaries due to rounding. The manual monotonicity fix ensures valid output MP4. |
| **Recommendation** | (a) This is already as fast as possible for the stream copy path — `rescale_ts` is a O(1) integer operation and the HashMap lookups are fast. (b) If profiling shows this as a hot spot, consider using arrays indexed by stream index instead of `HashMap` for `stream_routes`, `last_out_dts_by_stream`, and `last_out_pts_by_stream` since stream indices are dense small integers. |

---

### F9 — Thumbnail hash uses DefaultHasher which is SipHash (P4)

| Field | Value |
|-------|-------|
| **Location** | `companion_cache.rs` / `sdk_ffmpeg_output.rs:generate_thumbnail()` |
| **Severity | Low |
| **Description** | Thumbnail cache path is computed with `DefaultHasher` (SipHash-1-3). SipHash is cryptographically secure but 5× slower than a non-cryptographic hash like `FxHash` or `xxHash`. Since this is a file-path hash (not security-sensitive), the overhead is unnecessary. |
| **Code** | ```rust
let mut hasher = std::collections::hash_map::DefaultHasher::new();
video_path.hash(&mut hasher);
let hash = hasher.finish();
let thumb_path = cache_dir.join(format!("{:016x}.jpg", hash));
``` |
| **Why** | `DefaultHasher` is the default for `HashMap` and is chosen for DOS-resistance, but hash collisions on file paths are harmless (thumbnail just overwrites). |
| **Recommendation** | Switch to `rustc_hash::FxHasher` (used elsewhere in the codebase via `FxHashMap`) or `twox-hash::XxHash64` for ~5× faster hashing of path strings. |

---

### F10 — Post-process filter graph not used for NV12 encoders (P5)

| Field | Value |
|-------|-------|
| **Location** | `sdk_export.rs:850` — `attempt_export()` filter graph creation |
| **Severity | Low |
| **Description** | The post-processing filter graph only supports YUV420P input. Hardware encoders (NVENC, AMF, QSV) require NV12 input, so post-processing is silently disabled for hardware-accelerated exports. |
| **Code** | ```rust
if request.post_process_filters && encoder_pixel_format == ffmpeg::format::Pixel::YUV420P {
    // ... create filter graph
} else if request.post_process_filters {
    warn!("Skipping export post-processing filters because this encoder expects {:?} input frames", ...);
}
``` |
| **Why** | NV12 pixel format requires a different scaler pipeline. The filter graph would need a format conversion step (`scale_nv12=1`) before the filter chain. Since hardware encoders are preferred for speed, adding a pixel format conversion would partially negate the benefit. |
| **Recommendation** | (a) Add YUV420P→NV12 conversion at the end of the filter graph via `format=nv12` filter. (b) Or accept that post-processing is software-only and document this. (c) Move the filter graph to GPU (NVENC-based filters) when available. |

---

## Scoring

| Metric | Score | Notes |
|--------|-------|-------|
| **MP4 muxing throughput** | 7/10 | Interleaved writing is efficient; audio mixing is the bottleneck. |
| **Export decode speed** | 6/10 | Per-range decoder recreation adds overhead; no frame-level parallelism. |
| **Memory efficiency (clip save)** | 7/10 | Aggressive cleanup, but encoded audio holds full clip in memory. |
| **Memory efficiency (export)** | 8/10 | Frame reuse across ranges, explicit drops, temp file cleanup. |
| **I/O patterns** | 8/10 | Calibration files deleted; work dir cleaned on drop; no fsync abuse. |
| **Async correctness** | 9/10 | All blocking ops on dedicated threads (no tokio blocking). |
| **Codec detection overhead** | 5/10 | Scans all packets O(n); easy fix via propagating codec type. |
| **Calibration efficiency** | 4/10 | Two full exports for short clips can be a 3× multiplier on export time. |
| **Hash performance** | 6/10 | SipHash for file paths is overkill; cheap win with FxHasher. |

**Overall Score: 6.7 / 10** — The pipeline is well-engineered with good memory hygiene and correct async boundaries. The main wins come from reducing calibration overhead (F1), reusing resources across ranges (F2), and streaming audio encoding (F4).

---

## Key Recommendations (Priority Order)

1. **[P1] Skip or cache calibration for short clips / repeated exports** — Biggest potential savings on total export time.
2. **[P2] Hoist scaler, resampler, and filter graph outside the range loop** — Reduces per-segment allocation churn.
3. **[P3] Stream audio packets to the muxer incrementally** — Eliminates O(clip_duration) peak memory in audio path.
4. **[P3] Reuse audio mixing intermediate buffers** — Avoid 1 MB+ of per-chunk allocations.
5. **[P2] Propagate codec type through EncodedPacket** — Eliminates O(n) packet scan for codec detection.
6. **[P2] Fix memory hint drop** — Replace no-op `drop(Vec::with_capacity(0))` with actual FFmpeg pool hint.
7. **[P4] Switch to FxHasher for thumbnail cache hashing** — Tiny but trivial fix.
8. **[P5] Support NV12 in post-processing filter graph** — Enable filters for hardware-accelerated exports.
