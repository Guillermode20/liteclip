# Audio Pipeline Performance Analysis

**Date:** 2026-05-09
**Scope:** `crates/liteclip-core/src/capture/audio/` — WASAPI capture, RNNoise denoising, audio mixing, forwarding, level monitoring
**Files analyzed:** `mic.rs`, `system.rs`, `mixer.rs`, `manager.rs`, `level_monitor.rs`, `device_info.rs`, `pipelines/audio.rs`, `benches/audio_mixer.rs`

---

## Summary

The audio pipeline is well-structured with event-driven WASAPI capture, a dedicated RNNoise thread for noise suppression, a mixer with timestamp-based synchronization, and batched forwarding to the replay buffer. Several performance opportunities exist in the mixing hot path (per-sample float conversion, lack of SIMD), RNNoise queue management (redundant Vec compaction, per-frame copy overhead), and channel sizing. The pipeline avoids the most common pitfall (double-buffering resampling) by delegating SRC to WASAPI via `AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY`.

**Overall score: 7/10** — functionally sound, with headroom for latency reduction and CPU optimization.

---

## Hot Paths

1. **WASAPI capture + decode → channel** (system.rs:248-297, mic.rs:231-300) — runs every ~20ms, copies raw PCM from WASAPI buffers, packages into `EncodedPacket`, sends over crossbeam channel.
2. **RNNoise processing** (mic.rs:295-365, `RNNoiseProcessor::process`) — per-frame HP filter, `DenoiseState::process_frame` (480 samples / 10ms blocks), adaptive gate, mono→stereo broadcast, Vec compaction.
3. **Mixer matching + mixing** (mixer.rs:161-270, `process_matching_packets`) — decode bytes→i16, RMS calculation, per-sample float mixing, clamping, encode back to bytes.
4. **Forward loop packet batching** (pipelines/audio.rs:100-135) — select! receive, batch up to 32 packets, push to replay buffer.
5. **Level monitoring** (level_monitor.rs:47-110) — RMS scan of each packet's bytes, guarded by `gui_active` flag.

---

## Findings

### 1. Per-Sample Float Conversion in Mixer Lacks SIMD

| Field | Value |
|-------|-------|
| **Location** | `mixer.rs:225-244` — `process_matching_packets` float mixing loop |
| **Severity** | Medium |
| **Description** | The main mixing loop processes each sample individually: loads i16, divides by PCM_SCALE, multiplies by gain, adds the paired stream sample, applies balance, pushes to `mixed_float_buf`. On a stereo 20ms packet at 48kHz (1920 samples) this loop runs 1920 iterations doing 8+ float ops each. The 100ms packet (9600 samples) runs 9600 iterations. |
| **Code** | ```rust
for i in 0..max_samples {
    let system_sample = self.system_decode_buf[i];
    let mic_sample = self.mic_decode_buf[i];
    let system_scaled = (system_sample as f32 / PCM_SCALE) * system_gain;
    let mic_scaled = (mic_sample as f32 / PCM_SCALE) * mic_gain;
    let mixed = system_scaled + mic_scaled;
    let mut balanced = mixed;
    if i % 2 == 0 { balanced *= left_balance; }
    else { balanced *= right_balance; }
    self.mixed_float_buf.push(balanced);
}
``` |
| **Why it matters** | This loop is purely data-parallel. Without SIMD, each iteration pays instruction-fetch and branch overhead for balance checks. At 48k/20ms packets this is ~1920 iterations/call at ~50 calls/sec = ~96k iterations/sec. The inner loop is also split across two separate allocations (push to `mixed_float_buf`, then second loop pops and clamps). |
| **Recommendation** | Use `wide` crate or explicit `llvm_intrinsics` to process 4 or 8 stereo frames per SIMD lane. Combine the split loops: process, clamp, and write to `mixed_samples_buf` in a single pass. Use `chunks_exact_mut` to hoist the `i % 2` check outside the loop body with explicit left/right paths or a single `f32x4` lane split. |

---

### 2. Redundant RMS Calculation Over Full Float Mix Buffer

| Field | Value |
|-------|-------|
| **Location** | `mixer.rs:202-207`, `mixer.rs:229-231` |
| **Severity** | Low |
| **Description** | `calculate_rms_i16` is called on the decoded i16 buffers before mixing, then later the mixed_float_buf is iterated again for clamping. The RMS values are computed from pre-gain, pre-mix i16 data, then `update_smoothed_stream_levels` applies `system_user_gain` / `mic_user_gain` to approximate post-gain RMS. This is used only for `source_balance_gains` normalization, which is currently gated by `normalization_enabled` (default: off). The RMS calculation does a full scan of the i16 buffer with f64 accumulators. |
| **Code** | ```rust
let system_rms = calculate_rms_i16(&self.system_decode_buf) * system_user_gain;
let mic_rms = calculate_rms_i16(&self.mic_decode_buf) * mic_user_gain;
self.update_smoothed_stream_levels(has_system, system_rms, has_mic, mic_rms);
``` |
| **Why it matters** | When normalization is disabled (default), the entire RMS calculation + EMA update is wasted work. The function uses `f64` for the accumulator, but audio dynamic range is sufficiently represented by `f32` (24-bit mantissa covers 144dB). |
| **Recommendation** | Gate the entire RMS/EMA block behind `config.normalization_enabled` to skip it entirely when normalization is off (the common case). Switch to `f32` accumulation to reduce FPU pressure. |

---

### 3. RNNoise Queue Compaction Overhead on Every Process Call

| Field | Value |
|-------|-------|
| **Location** | `mic.rs:340-349` — `RNNoiseProcessor::compact_queue` |
| **Severity** | Medium |
| **Description** | `compact_queue` is called on both `in_buf` and `out_buf` after every `process()` call. It checks head position, does `copy_within` if head >= len/2, then truncates. For `out_buf`, this means after processing `N` frames and writing output, the head advances and memory is shifted. Since `process()` is called at ~20ms intervals (960 samples), the compaction runs ~50 times/sec. Both `in_buf` and `out_buf` use `Vec<f32>` with separate head pointers. |
| **Code** | ```rust
fn compact_queue(buf: &mut Vec<f32>, head: &mut usize) {
    if *head > 0 && *head >= buf.len() / 2 {
        let remaining = buf.len() - *head;
        buf.copy_within(*head.., 0);
        buf.truncate(remaining);
        *head = 0;
    }
    // ... shrink_to check
}
``` |
| **Why it matters** | `copy_within` is O(N) with N up to 64×480 = 30,720 f32 values (122KB). Combined across in_buf and out_buf, this can copy up to ~245KB/call, or ~12MB/sec at 50 calls/sec. The shrink_to check adds capacity comparison overhead. |
| **Recommendation** | Replace the head-pointer Vec scheme with a `Vec<f32>` and a dedicated ring buffer (e.g. `circular_buffer` crate or a small fixed-size array). Alternatively, use `VecDeque<[f32; AUDIO_FRAME_SIZE]>` to store frame-sized chunks, eliminating per-sample compaction entirely — only frame-level push/pop is needed. |

---

### 4. Per-Frame Heap-Allocated Frame Copies in RNNoise

| Field | Value |
|-------|-------|
| **Location** | `mic.rs:358-367` — RNNoise frame processing loop |
| **Severity** | Medium |
| **Description** | Each 480-sample RNNoise frame does: `self.frame_in.copy_from_slice(...)`, calls `state.process_frame()` (which writes to `self.frame_out`), then `self.out_buf.extend_from_slice(self.frame_out.as_slice())`. The `frame_in` and `frame_out` are `Box<[f32; AUDIO_FRAME_SIZE]>` — heap-allocated fixed arrays. While these avoid per-call allocation, the copy_from_slice is still a 1.9KB memcpy per frame. |
| **Code** | ```rust
self.frame_in.copy_from_slice(&self.in_buf[start..start + AUDIO_FRAME_SIZE]);
let speech_prob = self.state.process_frame(self.frame_out.as_mut_slice(), self.frame_in.as_slice());
// ...
self.out_buf.extend_from_slice(self.frame_out.as_slice());
``` |
| **Why it matters** | At 48kHz with 20ms packets (960 samples), the RNNoise loop runs 2 iterations per process() call → 100 iterations/sec. Each iteration does ~1.9KB in + ~1.9KB out = ~3.8KB/iteration = ~380KB/sec of redundant copying through the frame buffers. The `process_frame` call itself is the dominant cost (RNNoise is ~60 MFLOPS), but the frame copy adds ~1-2% overhead. |
| **Recommendation** | If `nnnoiseless::DenoiseState::process_frame` can accept non-contiguous input, pass slices directly from `in_buf` to avoid the `frame_in` copy. Otherwise, keep the architecture but consider processing multiple frames in one batch to amortize the copy cost. |

---

### 5. Crossbeam Channel Sizing and Blocking Potential

| Field | Value |
|-------|-------|
| **Location** | `mic.rs:63` → `bounded(128)`, `system.rs:55` → `bounded(64)`, `manager.rs:34` → `bounded(64)`, `mic.rs:197` → `bounded(64)` (RNNoise raw frame queue) |
| **Severity** | Low |
| **Description** | Channel capacities are modest: system capture 64, mic capture 128, RNNoise raw frames 64, manager output 64. At 20ms packet intervals, each channel fills at ~50 packets/sec. A 64-capacity channel holds ~1.28s of audio when blocked. The mic capture thread blocks when `noise_tx.send(raw)` is full (logs `"RNNoise input queue congested"` after 5ms wait). The forward loop uses `select!` with `default(wait_timeout)` for non-blocking receive. |
| **Code** | ```rust
// mic.rs:62
let (packet_tx, packet_rx) = bounded(128);
// system.rs:54
let (packet_tx, packet_rx) = bounded(64);
// manager.rs:33
let (packet_tx, packet_rx) = crossbeam::channel::bounded(64);
// mic.rs:196 (noise thread)
let (tx, rx) = crossbeam::channel::bounded::<RawMicFrame>(64);
``` |
| **Why it matters** | Under load spikes (e.g., mixer stalling due to encoder backpressure), the capped channels cause backpressure that propagates to capture threads. The mic capture path has a 5ms stall detection threshold and logs warnings. The system capture path has no such monitoring — it silently blocks on `packet_tx.send()`. Channel `128` for mic raw output is the largest but still only ~2.56s of buffering. |
| **Recommendation** | Increase mic output to `bounded(256)` (5.12s buffer) and system output to `bounded(128)` (2.56s) to tolerate longer encoder stalls without backpressure. Add `try_send` with fallback for the system capture path to match the mic path's monitoring. Consider a shared telemetry counter in `WasapiAudioManager` to log channel pressure across all stages. |

---

### 6. EncodedPacket Decode Produces Temporary BytesMut

| Field | Value |
|-------|-------|
| **Location** | `mixer.rs:290-298` — `decode_packet_into` |
| **Severity** | Low |
| **Description** | `decode_packet_into` iterates `packet.data.chunks_exact(2)` to produce `i16` samples. The `EncodedPacket.data` is a `Bytes` (ref-counted cheap clone), but decoding into i16 requires reading every byte. The decoded i16 buffers (`system_decode_buf`, `mic_decode_buf`) are re-used across calls, avoiding re-allocation. However, the output pack in `process_matching_packets` builds a new `BytesMut`, extends with `sample.to_le_bytes()`, then freezes it — this is the dual of the decode and could be fused. |
| **Code** | ```rust
fn decode_packet_into(packet: &EncodedPacket, buffer: &mut Vec<i16>) {
    buffer.clear();
    buffer.reserve(packet.data.len() / 2);
    buffer.extend(
        packet.data.chunks_exact(2)
            .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]])),
    );
}
``` |
| **Why it matters** | Each mixed output goes decode→float→process→i16→bytes. The encode step produces `max_samples * 2` bytes using `extend_from_slice(&sample.to_le_bytes())` which is a tight loop. Decoding and encoding could be fused with the float mixing to skip the intermediate `mixed_samples_buf` Vec entirely, saving one full Vec iteration and one allocation. |
| **Recommendation** | Fuse the float mixing and i16 quantization into a single pass. Instead of writing to `mixed_float_buf` then reading back to produce `mixed_samples_buf`, write directly to `output_buffer` as little-endian bytes after mixing each sample pair. This eliminates the `mixed_float_buf` Vec and the second iteration. |

---

### 7. WASAPI SRC Delegated to Driver — No Visibility or Control

| Field | Value |
|-------|-------|
| **Location** | `system.rs:130`, `mic.rs:118` — `AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY` |
| **Severity** | Low |
| **Description** | Both system and mic capture request 48kHz / 16-bit via WASAPI format negotiation with `AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY` and `AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM`. This tells WASAPI to perform any needed sample rate conversion and format conversion transparently. Mic sources typically produce 44.1kHz or 48kHz, system loopback is always the device's output rate (usually 48kHz or 96kHz on modern hardware). The conversion quality is "default," which is medium-quality linear interpolation — not the highest quality polyphase resampling. |
| **Code** | ```rust
let stream_flags = AUDCLNT_STREAMFLAGS_EVENTCALLBACK
    | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
    | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
``` |
| **Why it matters** | The SRC quality and latency are controlled by the WASAPI audio engine and are not observable from the Rust code. On underpowered or virtual audio devices, the engine's SRC may introduce unexpected latency or quality degradation. There is no mechanism to switch to `AUDCLNT_STREAMFLAGS_SRC_HIGH_QUALITY` or to use a software resampler (e.g., `rubato`) for consistent quality across all hardware. |
| **Recommendation** | For highest consistency, consider adding an optional software resampler path (e.g., `rubato` crate) that captures at the device's native rate and explicitly resamples to 48kHz. This gives deterministic quality and latency at the cost of ~5-10% additional CPU. Gate this behind a config option. |

---

### 8. Non-Blocking Mixer Input Without Backpressure Signal

| Field | Value |
|-------|-------|
| **Location** | `manager.rs:99-130` — `forward_loop` try_recv polling |
| **Severity** | Medium |
| **Description** | The forward loop polls both system and mic channels with `try_recv()` in a spin-like pattern. When neither has data, it falls through to `crossbeam::channel::select!` with a 20ms `default` timeout. This means the mixer is effectively polled at ~50Hz with up to 20ms of additional latency per iteration if only one stream has data. When both streams are active, the mixer receives packets one-at-a-time per iteration. |
| **Code** | ```rust
// Manager: try_recv loop
if system_packet.is_none() {
    if let Some(rx) = system_rx.as_ref() {
        match rx.try_recv() { ... }
    }
}
if mic_packet.is_none() {
    if let Some(rx) = mic_rx.as_ref() {
        match rx.try_recv() { ... }
    }
}
// ... only after both checked, or if both none, falls through to select!
if system_packet.is_some() || mic_packet.is_some() {
    let mixed_packets = mixer.mix_packets(system_packet.take(), mic_packet.take());
    // ...
} else {
    // select! with 20ms timeout
}
``` |
| **Why it matters** | The polling pattern adds variable latency: when only one stream is active, each packet waits up to the select! timeout (20ms) before being mixed and forwarded. Additionally, the mixer's `mix_packets` is called with one packet per stream per call, even when multiple packets are queued — this misses opportunities for batch mixing. |
| **Recommendation** | Use `try_recv` in a drain loop to collect all available packets from both channels before calling `mix_packets`. This reduces the number of `mix_packets` calls and allows the mixer to batch its internal sync operations. Consider removing the blocking `select!` fallback in favor of `recv_timeout` on a single notification channel, or use a merged channel approach. |

---

### 9. Level Monitor RMS Uses f64 Accumulator Unnecessarily

| Field | Value |
|-------|-------|
| **Location** | `level_monitor.rs:88-120` — `calculate_levels_stereo`, `calculate_levels_stereo_bytes` |
| **Severity** | Low |
| **Description** | Both RMS functions accumulate squared sample values in `f64`. Over `sum_left += (sample as f64).powi(2)` with `i16` input, the maximum value per sample is 32768² ≈ 1.07×10⁹. A 20ms stereo packet has 960 samples per channel, summing to ~1.03×10¹² — well within `f32` range (max ~3.4×10³⁸, 24-bit mantissa ≈ 16.7M distinct values, sufficient for this use case). |
| **Code** | ```rust
let mut sum_left: f64 = 0.0;
let mut sum_right: f64 = 0.0;
for chunk in samples.chunks_exact(2) {
    sum_left += (chunk[0] as f64).powi(2);
    sum_right += (chunk[1] as f64).powi(2);
}
``` |
| **Why it matters** | f64 operations are 2-4× slower on modern x64 CPUs than f32 (wider SIMD, less memory bandwidth). For a function called 50 times/sec per active stream, this is negligible, but in combination with the mixer's own RMS calculation, the total f64 overhead adds up. |
| **Recommendation** | Switch to `f32` for RMS accumulation. The 24-bit mantissa provides ~144dB dynamic range, more than enough for perceptual level metering. |

---

### 10. Thread Priority Mismatch: Noise Thread at ABOVE_NORMAL While Capture Is Same

| Field | Value |
|-------|-------|
| **Location** | `mic.rs:149` (capture thread priority), `mic.rs:309` (noise thread priority) — both `THREAD_PRIORITY_ABOVE_NORMAL` |
| **Severity** | Low |
| **Description** | Both the WASAPI capture thread and the RNNoise processing thread are set to `THREAD_PRIORITY_ABOVE_NORMAL`. This means they compete equally for CPU. If the noise thread starves the capture thread (or vice versa), audio glitches may occur. Ideally, the capture thread should be higher priority than the processing thread to avoid dropped WASAPI buffers. |
| **Code** | ```rust
// Capture thread:
SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_ABOVE_NORMAL);
// Noise thread:
SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_ABOVE_NORMAL);
``` |
| **Why it matters** | WASAPI in event-driven mode provides exactly one buffer duration window (~20ms) to drain the buffer before glitches. The RNNoise thread has no such real-time constraint — a delayed frame just means slight output latency. Equal priority risks the CPU-intensive noise thread preempting the latency-sensitive capture thread. |
| **Recommendation** | Set the capture thread to `THREAD_PRIORITY_HIGHEST` (one notch above the noise thread) to ensure the WASAPI buffer is always drained on time. Keep the noise thread at `THREAD_PRIORITY_ABOVE_NORMAL`. |

---

## Scoring

| Category | Score (1-10) | Notes |
|----------|-------------|-------|
| **Sample rate conversion** | 8 | Delegated to WASAPI with default quality; consistent but unconfigurable. No explicit resampling code to maintain. |
| **Noise suppression** | 6 | RNNoise is efficient per-frame, but queue management (Vec compaction, head pointer) adds overhead. Frame-copy architecture is robust but not optimal. |
| **Audio mixing** | 6 | Functionally correct with good sync logic, but lacks SIMD, has split-loop pattern, and runs double RMS scans when normalization is off. |
| **Buffer management** | 7 | Channel sizes acceptable but could be tuned. Mixer eviction at 32 packets is generous (640ms). No memory pooling for audio buffers. |
| **WASAPI threading** | 7 | Event-driven with ABOVE_NORMAL priority is good. Missing priority differentiation between capture and processing threads. System capture has no retry logic. |
| **Overall** | **7** | Solid foundation. Key wins: SIMD in mixer, RNNoise queue ring buffer, fused mixing loop, tiered thread priorities. |

---

## Recommendations (Priority Order)

1. **High** — SIMD-vectorize the mixer's inner loop (mixer.rs:225-244) using `f32x4` lanes for 4 stereo frames at once. Fuse with quantization to eliminate `mixed_float_buf`.
2. **High** — Replace RNNoise Vec+head-pointer queues with `VecDeque<[f32; AUDIO_FRAME_SIZE]>` to eliminate per-call compaction (`compact_queue`).
3. **Medium** — Gate RMS calculation behind `normalization_enabled` in the mixer to skip it when unused (default).
4. **Medium** — Differentiate thread priorities: capture thread → `HIGHEST`, noise thread → `ABOVE_NORMAL`.
5. **Medium** — Convert mixer forward loop to drain-loop `try_recv` pattern to batch multiple packets per `mix_packets` call.
6. **Low** — Increase channel capacities: system `64→128`, mic `128→256` for deeper tolerance of encoder backpressure.
7. **Low** — Switch RMS accumulators from `f64` to `f32` in both mixer and level_monitor.
8. **Low** — Benchmarks only cover mixer creation and empty mixing; add benchmarks for full mixing with realistic packet sizes (960, 4800, 9600 samples) and RNNoise processing latency.
