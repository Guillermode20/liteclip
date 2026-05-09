# Report 04: Encoding Layer Performance Analysis

**Date:** 2026-05-09
**Scope:** `crates/liteclip-core/src/encode/` — FFmpeg hardware encoders (NVENC, AMF, QSV), software encoder (libx265, MJPEG), CLI pipe encoder, encoder manager/spawner, packet pipeline.

---

## Summary

The encoding layer is well-architected with strong foundations: GPU texture transport via D3D11 shared device, batch packet handling, and bounded channels throughout. Key strengths include the zero-copy `Bytes`-based packet pipeline and the lock-free SPMC ring buffer integration. However, significant optimization opportunities exist in hardware encoder probe overhead, per-frame reinitialization checks, QSV per-frame mapping cost, and thread management (no affinities, no real-time scheduling). The probe path does full encoder open/close for each of three codecs during startup. The packet batch drain runs **twice per frame** in the hot path. Software encoder uses `libx265 ultrafast` which is CPU-heavy even at fastest preset (~2-4x slower than hardware encode).

**Score: 7.2/10** (good foundation, moderate gains available in initialization path and batch/timing tuning)

---

## Hot Paths

### 1. Per-Frame Encode Hot Path (every captured frame)
```
spawn_encoder_with_receiver (function.rs)
  → frame_rx.recv_timeout(frame_recv_timeout)
  → encoder.encode_frame(&frame)                               // main encode work
    → FfmpegEncoder::encode_frame (mod.rs)
      → reinit check (encoder is None / resolution changed / GPU mode changed)   // CHECK
      → encode_gpu_frame() OR init_encoder + software path                         // WORK
      → drain_encoder_packets()                                                     // READ BACK
  → drain_ready_packets()                                            // SECOND DRAIN
  → batch flush (if age or size threshold exceeded)
```
`drain_ready_packets` is called **every iteration** (after timeout, after each frame, and in burst loop). This means `packet_rx.try_recv()` is polled 2-3× per frame.

### 2. Auto-Detect Hot Path (once at startup)
```
detect_hardware_encoder()
  → probe_encoder_available("hevc_nvenc")   // opens full encoder with 320x240
  → probe_encoder_available("hevc_amf")     // opens full encoder with 320x240
  → probe_encoder_available("hevc_qsv")     // opens full encoder with 320x240
```
Each probe does: `find_by_name → new_with_codec → set 7 parameters → open_with(options)` — this is **3 full encoder init attempts** at startup.

### 3. QSV Per-Frame Path (cold path)
```
encode_qsv_gpu_frame()
  → prepare_hw_frame()           // get buffer, set texture ptr (on shared device)
  → av_hwframe_ctx_alloc()       // ALLOCATE new QSV frame EVERY frame
  → av_hwframe_map()             // MAP D3D11->QSV EVERY frame
  → avcodec_send_frame()
  → av_frame_free()              // FREE QSV frame EVERY frame
```
NVENC/AMF use a **reusable** HW frame; QSV allocates+maps+frees per frame.

---

## Findings

### F-01: Hardware encoder probe opens full encoder contexts at startup

| Field | Value |
|-------|-------|
| **Location** | `functions.rs:165-196` — `detect_hardware_encoder()` |
| **Severity** | Medium (startup, not per-frame) |
| **Category** | Initialization overhead |
| **Impact** | Adds 50-300ms to startup depending on GPU driver response time |

**Description:**
`detect_hardware_encoder()` probes NVENC, AMF, and QSV sequentially by calling `probe_encoder_available()` for each. Each probe fully initializes an FFmpeg encoder context with `find_by_name → new_with_codec → set_width/set_height/set_format/set_bitrate/set_gop → open_with(options)`. The probe uses 320×240 resolution and hardcoded options. The second and third probes only run if earlier ones fail, but on systems with multiple GPUs (e.g., NVIDIA dGPU + Intel iGPU), all three could be attempted.

**Code:**
```rust
// functions.rs:165-196
pub fn detect_hardware_encoder() -> HardwareEncoder {
    if probe_encoder_available("hevc_nvenc") { return HardwareEncoder::Nvenc; }
    if probe_encoder_available("hevc_amf")   { return HardwareEncoder::Amf; }
    if probe_encoder_available("hevc_qsv")   { return HardwareEncoder::Qsv; }
    HardwareEncoder::None
}
```

**Why this is suboptimal:**
- Each probe does `encoder.open_with(options)` which triggers full FFmpeg encoder init including GPU driver interaction (D3D11 device creation, driver negotiation)
- Probing is serial: no parallelism, and no early exit via a lightweight "codec exists" check
- The probe itself has no timeout — a hung GPU driver response blocks startup

**Recommendation:**
1. Add a lightweight pre-check using `ffmpeg::encoder::find_by_name()` before the full `open_with()` probe — if the codec name doesn't exist in the linked FFmpeg build, skip the heavy probe entirely (this is already done partially, but the open_with is still the bottleneck)
2. Consider running probes in parallel across a thread pool or using `rayon::scope`
3. Cache probe results so re-detection (config reload) doesn't re-probe
4. Consider a lightweight D3D11-based GPU vendor check before probing (check `IDXGIAdapter::GetDesc` for vendor ID) to skip non-existent vendors entirely

---

### F-02: Per-frame `encoder.is_none()` / `last_input_res` / `needs_transport_reinit` check in hot path

| Field | Value |
|-------|-------|
| **Location** | `ffmpeg/mod.rs:124-167` — `FfmpegEncoder::encode_frame()` |
| **Severity** | Low |
| **Category** | Per-frame overhead |
| **Impact** | <1µs per frame, but adds branch complexity |

**Description:**
Every frame checks `self.encoder.is_none()`, whether resolution changed, and whether GPU transport mode changed. After initialization, all three checks are trivially false. This is negligible for steady-state but adds unnecessary branch complexity and cache pressure to the critical path.

**Code:**
```rust
if self.encoder.is_none()
    || self.last_input_res != (frame.resolution.0, frame.resolution.1)
    || needs_transport_reinit
{
    // ... reinitialization logic (often 20+ lines)
}
```

**Why this is suboptimal:**
- The reinit branch is ~40 lines of code that sits in the icache of the hot path
- On modern x86 CPUs, a predictable branch miss is ~10-15 cycles, so this is genuinely tiny — but the code volume impacts I-cache

**Recommendation:**
- Restructure into a separate `ensure_encoder_ready(&mut self, frame: &CapturedFrame)` method that returns early if no reinit needed
- Or use a tri-state enum (`Uninitialized | Ready { width, height, gpu_mode } | NeedsReinit`) to make the check a single match
- Low priority; mainly a readability/cleanliness improvement

---

### F-03: `drain_ready_packets()` polled 2-3× per frame in hot path

| Field | Value |
|-------|-------|
| **Location** | `functions.rs:283-395` — `spawn_encoder_with_receiver()` |
| **Severity** | Medium |
| **Category** | Batch/Timing overhead |
| **Impact** | 2-3 `try_recv()` syscalls per frame at 60fps = 120-180 wakes/sec |

**Description:**
The encoder thread's main loop calls `drain_ready_packets()`:
1. At the top of every loop iteration (before `recv_timeout`)
2. Inside `encode_one()` after `encoder.encode_frame()`
3. In the burst loop after each additional frame encode
4. On timeout (after `RecvTimeoutError::Timeout`)

Each invocation calls `try_recv()` on the packet channel in a tight loop. In steady state with hardware encoding, most of these return empty immediately (latency between encode and packet receipt).

**Code:**
```rust
loop {
    total_forwarded_packets = total_forwarded_packets.saturating_add(
        drain_ready_packets(&packet_rx, &buffer, ...)  // #1
    );
    match frame_rx.recv_timeout(frame_recv_timeout) {
        Ok(frame) => {
            encode_one(frame)?;  // calls drain_ready_packets #2 internally
            for _ in 1..MAX_FRAME_BURST {
                let Ok(frame) = frame_rx.try_recv() else { break; };
                encode_one(frame)?;  // calls drain_ready_packets #3
            }
        }
        Err(Timeout) => {
            drain_ready_packets(&packet_rx, ...);  // #4
            flush_packet_batch(...);
        }
    }
}
```

**Why this is suboptimal:**
- `try_recv()` on an empty channel still requires atomic operations and memory barriers (crossbeam channel internal: atomic load on the tail pointer)
- At 60fps, the burst loop typically gets 2-3 frames, meaning `try_recv()` on packet channel runs 3-4 times per frame
- Each `try_recv()` call includes a SeqCst fence (crossbeam's fast path)

**Recommendation:**
- Move packet draining to only run **after** the burst loop, not after every individual frame
- Only drain at the top of the outer loop iteration — the extra drains inside `encode_one()` are usually redundant since the encoder hasn't produced new packets yet (they come from `drain_encoder_packets` which is called at the end of `encode_frame`)
- Consider using a separate notification mechanism (e.g., `std::sync::mpsc::Sender::send` that blocks until received, or a condition variable) instead of polling

---

### F-04: QSV per-frame `av_hwframe_ctx_alloc` + `av_hwframe_map` + `av_frame_free`

| Field | Value |
|-------|-------|
| **Location** | `ffmpeg/qsv.rs:146-205` — `encode_qsv_gpu_frame()` |
| **Severity** | High |
| **Category** | Per-frame allocation |
| **Impact** | ~5-20µs per frame for allocation + mapping + free; includes kernel transitions |

**Description:**
NVENC and AMF use a **reusable** HW frame (`hw_context.reusable_hw_frame`), reusing the same FFmpeg frame for every encode operation. QSV, in contrast, allocates a **new** `AVFrame` every frame via `av_frame_alloc()`, performs `av_hwframe_map()` to map the D3D11 surface to QSV, then frees the frame with `av_frame_free()`. This is 3 heap operations + device synchronization per frame.

**Code:**
```rust
// qsv.rs:150-165 (inside encode_qsv_gpu_frame)
let mut qsv_frame = ffmpeg::ffi::av_frame_alloc();                    // HEAP ALLOC
let map_res = ffmpeg::ffi::av_hwframe_map(qsv_frame, d3d11_frame, 0); // MAPPING
// ... set PTS, pict_type, key_frame
let send_result = ffmpeg::ffi::avcodec_send_frame(encoder.as_mut_ptr(), qsv_frame);
ffmpeg::ffi::av_frame_free(&mut qsv_frame);                             // FREE
```

**Why this is suboptimal:**
- `av_frame_alloc()` performs heap allocation and zero-initialization of a ~500+ byte struct
- `av_hwframe_map()` likely involves a D3D11 device operation (driver call)
- `av_frame_free()` performs heap deallocation and refcount management
- This pattern repeats 30-60 times per second (at 30-60fps)
- NVENC and AMF path reuse: `hw_context.reusable_hw_frame` is allocated once and reused

**Recommendation:**
- Reuse a pre-allocated QSV frame (allocate once, store as `self.qsv_mapped_frame` or similar)
- Only call `av_hwframe_map()` when the source texture changes (unlikely frame-to-frame in steady capture)
- Or: investigate if QSV can accept D3D11 textures directly without the mapping step (using `AV_PIX_FMT_D3D11` instead of `AV_PIX_FMT_QSV`)
- This is likely the #1 performance bottleneck on Intel GPU systems

---

### F-05: D3D11 hardware context creation re-encodes device pointer on every init

| Field | Value |
|-------|-------|
| **Location** | `ffmpeg/context.rs:84-140` — `create_d3d11_hardware_context_from_device()` |
| **Severity** | Medium |
| **Category** | Initialization overhead |
| **Impact** | ~5-15ms per hardware encoder init (called once per recording session) |

**Description:**
Each time a hardware encoder is initialized, `create_d3d11_hardware_context_from_device` is called. This function:
1. Allocates `AVHWDeviceContext` via `av_hwdevice_ctx_alloc(AV_HWDEVICE_TYPE_D3D11VA)` — creates a fresh FFmpeg-side D3D11 device wrapper
2. Calls `GetImmediateContext()` on the capture device
3. Pokes raw D3D11 device pointers into the `AvD3d11vaDeviceContext` struct's `device` and `device_context` fields
4. Calls `av_hwdevice_ctx_init()` to finalize the context
5. Allocates and initializes a hardware frames context via `create_hw_frames_ctx_with_pool_size()`
6. Allocates a reusable AVFrame via `av_frame_alloc()`

**Code:**
```rust
// core of create_d3d11_hardware_context_from_device
let hw_device_ctx = (*device_ctx_ref).data as *mut ffmpeg::ffi::AVHWDeviceContext;
let d3d11_ctx = (*hw_device_ctx).hwctx as *mut AvD3d11vaDeviceContext;
let context = device.GetImmediateContext()...;
(*d3d11_ctx).device = ffmpeg_device.as_raw() as *mut _;
(*d3d11_ctx).device_context = ffmpeg_context.as_raw() as *mut _;
// ...
ffmpeg::ffi::av_hwdevice_ctx_init(device_ctx_ref);
// ...
let frames_ctx_ref = Self::create_hw_frames_ctx_with_pool_size(...)?;
let reusable_hw_frame = ffmpeg::ffi::av_frame_alloc();
```

**Why this is suboptimal:**
- Creating and initializing an FFmpeg HW device context involves internal allocations, D3D11 device reference management, and driver queries
- The device pointer is set from the capture device's `ID3D11Device` — if the same device is reused across recording sessions, this context should be cached
- `GetImmediateContext()` returns a COM interface with `AddRef` internally

**Recommendation:**
- Cache the `D3d11HardwareContext` across encoder reinitializations when the source device is the same
- The context could be stored at the engine/pipeline level and passed to the encoder thread
- If resolution changes but device is the same, only the frames context needs recreation (not the device context)

---

### F-06: Software encoder uses `libx265 ultrafast` — heavy even at fastest preset

| Field | Value |
|-------|-------|
| **Location** | `ffmpeg/options.rs:145` — `apply_software_options()`, `software_preset()` |
| **Severity | Medium (CPU encode path) |
| **Category** | Preset choice |
| **Impact** | libx265 ultrafast can consume 2-4 full CPU cores for 1080p60 |

**Description:**
The software HEVC encoder (`libx265`) uses `ultrafast` preset by default (scaling to `superfast`/`veryfast` for higher quality presets). Even `ultrafast` is computationally expensive for real-time 1080p60 HEVC encoding on CPU. The encoder also uses `zerolatency` tuning and no B-frames, which further increase bitrate at a given quality level.

**Code:**
```rust
// options.rs
pub(super) fn apply_software_options(&self, options: &mut ffmpeg_next::Dictionary<'_>) {
    options.set("preset", self.software_preset());
    options.set("tune", "zerolatency");
    options.set("bf", "0");
    // ...
}
pub(super) fn software_preset(&self) -> &'static str {
    match self.config.quality_preset {
        QualityPreset::Performance => "ultrafast",
        QualityPreset::Balanced => "superfast",
        QualityPreset::Quality => "veryfast",
    }
}
```

**Why this is suboptimal:**
- libx265 `ultrafast` is the fastest x265 preset but still CPU-intensive for real-time use (2-4 cores at 1080p60)
- No thread count override — x265 auto-detects thread count and may oversubscribe the system
- No explicit `pools` parameter to limit thread count
- Consider libx264 as a lighter alternative for software encoding (especially at ultrafast, x264 is ~2x faster than x265)

**Recommendation:**
- Add `pools` parameter to limit threads (e.g., `pools=4` or `pools=<num_physical_cores/2>`)
- Consider offering libx264 as an alternative software encoder for lower-end systems
- Document expected CPU load for software encoding in the settings UI
- In the long term, consider adding a MJPEG fallback (which `sw_encoder.rs` implements but the FFmpeg path doesn't use)

---

### F-07: Software encoder thread count tied to `available_parallelism()` without affinity

| Field | Value |
|-------|-------|
| **Location** | `sw_encoder.rs:117-126` — `SoftwareEncoder::new()` |
| **Severity** | Low-Medium |
| **Category** | Thread management |
| **Impact** | Up to 8 threads contending with capture/game threads |

**Description:**
The software encoder (`sw_encoder.rs`) creates 2-8 worker threads based on `thread::available_parallelism()`. Workers use `std::thread::spawn` with no priority, no affinity, and no name (unlike the main encoder thread which is named "encoder" with stack size set). Workers compete with the game being recorded for CPU time.

**Code:**
```rust
// sw_encoder.rs:117-126
let num_workers = thread::available_parallelism()
    .map(|n| n.get())
    .unwrap_or(4)
    .clamp(2, 8);
// ...
for id in 0..num_workers {
    let rx = worker_rx.clone();
    let tx = packet_tx.clone();
    worker_threads.push(thread::spawn(move || {  // No name, no priority, no affinity
```

**Why this is suboptimal:**
- Workers have default priority (NORMAL) — they compete with the game being recorded
- No thread naming makes debugging/ profiling harder (e.g., in WinDBG, PerfView)
- 8 threads doing JPEG compression + bilinear scaling can cause significant cache contention on the BGRA source data

**Recommendation:**
- Set thread names via `std::thread::Builder::new().name("sw-encoder-0")...`
- Set thread priority to `BELOW_NORMAL` (matching `set_encoder_thread_priority` in the encoder thread)
- Add optional affinity pinning to non-performance cores (e.g., E-cores on hybrid architectures)
- Clamp workers to `num_physical_cores - 1` (not `available_parallelism()` which includes SMT threads)
- Consider a work-stealing or single-threaded mode for low-core-count systems

---

### F-08: Main encoder thread priority is `BELOW_NORMAL` but no affinity pinning

| Field | Value |
|-------|-------|
| **Location** | `functions.rs:103-117` — `set_encoder_thread_priority()` |
| **Severity | Low |
| **Category** | Thread management |
| **Impact** | No latency isolation for encoding threads |

**Description:**
The main encoder thread priority is set to `THREAD_PRIORITY_BELOW_NORMAL` via `set_encoder_thread_priority()`. However, there is no CPU affinity set. The thread may run on any core, including high-performance cores that the capture thread or game thread also uses.

**Code:**
```rust
fn set_encoder_thread_priority() {
    #[cfg(windows)]
    {
        unsafe {
            SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
        }
    }
}
```

**Why this is suboptimal:**
- No `SetThreadAffinityMask` or `SetThreadIdealProcessor` call to isolate encoder thread to a specific core or core group
- On hybrid architectures (Intel P-core/E-core), the encoder could migrate to a P-core and compete with the game
- BELOW_NORMAL priority means the encoder yields to foreground apps, but it can still cause LLC misses on P-cores

**Recommendation:**
- Pin encoder threads to E-cores or a dedicated core group using `SetThreadAffinityMask`
- After priority setting, add an optional affinity call (e.g., `SetThreadIdealProcessor` for scheduling hint)
- Consider using `THREAD_PRIORITY_LOWEST` for software encoding paths where frames can be dropped
- Document thread placement strategy for future tuning

---

### F-09: `pending_packet_timestamps` VecDeque grows and shrinks for every frame

| Field | Value |
|-------|-------|
| **Location** | `ffmpeg/mod.rs:209-212` — `encode_frame()` |
| **Severity** | Low |
| **Category** | Memory management |
| **Impact** | One `push_back` + possible `pop_front` per frame (minimal, but VecDeque can resize) |

**Description:**
Every frame pushes the frame timestamp into `pending_packet_timestamps` via `push_back()`. If the deque exceeds 512 entries, the oldest is popped. The deque is initialized with `VecDeque::with_capacity(256)`.

**Code:**
```rust
self.pending_packet_timestamps.push_back(frame.timestamp);
if self.pending_packet_timestamps.len() > 512 {
    self.pending_packet_timestamps.pop_front();
}
```

**Why this is suboptimal:**
- VecDeque resizing from 256 to 512 capacity triggers a reallocation + copy of all elements
- Each `pop_front()` shifts the ring's head pointer but triggers no reallocation — this is fine
- The deque is only consumed in `drain_encoder_packets()` via `pop_front()`, meaning during steady state it maintains ~1 entry at a time (one push per frame, one pop per packet drain)
- The 512 cap seems high — it will never be reached at 60fps if packets drain normally
- One extra branch (`len() > 512`) in the per-frame hot path

**Recommendation:**
- Reduce initial capacity to 64 (more than enough for typical encode pipeline depth)
- The 512 cap and branch can be replaced with a `debug_assert` or removed entirely
- Consider using a small vector optimization or a fixed-size array ring buffer for this

---

### F-10: Batch flush uses `Instant::now()` on every packet batch push

| Field | Value |
|-------|-------|
| **Location** | `functions.rs:347` — `flush_packet_batch()` |
| **Severity** | Low |
| **Category** | Timing overhead |
| **Impact** | `Instant::now()` syscall (QueryPerformanceCounter) on every batch flush |

**Description:**
Every time a packet batch is flushed to the ring buffer, `Instant::now()` is called to record the flush time. This is used by `drain_ready_packets()` to check if `MAX_PACKET_BATCH_AGE_MS` (75ms) has elapsed.

**Code:**
```rust
fn flush_packet_batch(..., last_packet_flush: &mut Instant) {
    buffer.push_batch(packet_batch.drain(..));
    *flush_batches += 1;
    *last_packet_flush = Instant::now();  // QPC call
}
```

**Why this is suboptimal:**
- `Instant::now()` on Windows calls `QueryPerformanceCounter()` — a relatively expensive syscall (~50-200ns depending on implementation)
- At 60fps with MAX_PACKET_BATCH_LEN=32 and typical packet counts, this could be called 2-10 times per second
- Additionally, the age check in `drain_ready_packets()` calls `last_packet_flush.elapsed()` which also calls `Instant::now()` internally

**Recommendation:**
- Use a simpler mechanism: just flush every N packets without a time guard (the batch age was introduced to avoid starvation on idle)
- Or: only check age on timeout path (where we know packets might be stale)
- Replace `Instant` with a simpler tick counter (e.g., just count frames since last flush)
- Or use `std::time::Instant::now()` lazily — cache the current time and only refresh it every 50ms

---

### F-11: BMP → JPEG BGRA→RGB conversion in `sw_encoder.rs` uses floating-point bilinear scaling

| Field | Value |
|-------|-------|
| **Location** | `sw_encoder.rs:55-120` — `bgra_to_jpeg_reuse()`, bilinear scaling branch |
| **Severity** | Medium |
| **Category** | CPU efficiency |
| **Impact** | ~1-3ms per 1080p frame for scaling with float arithmetic |

**Description:**
The bilinear scaling path in `bgra_to_jpeg_reuse()` uses `f32` arithmetic for every pixel interpolation. Each output pixel requires 4 texture fetches, 4 float subtractions, 2 float multiplications, and 1 float addition per channel (3 channels). For a 1080p → 1080p scale (1:1), this path is not taken (fast path runs). But for any resolution change, the float path runs for all pixels.

**Code:**
```rust
// sw_encoder.rs ~line 80-120 (bilinear scaling branch)
for dst_y in 0..out_h {
    for dst_x in 0..out_w {
        let src_x = (dst_x as f32 + 0.5) * x_ratio - 0.5;
        // ... f32 arithmetic for 4 texel fetches and interpolation
        for c in 0..3 {
            let v00 = bgra[i00 + src_c] as f32;
            let v10 = bgra[i10 + src_c] as f32;
            // ... float interpolation
            let v = v_top + (v_bot - v_top) * y_frac;
            rgb_buf[di + c] = v.clamp(0.0, 255.0) as u8;
        }
    }
}
```

**Why this is suboptimal:**
- Float-to-int and int-to-float conversion for each pixel is expensive
- No SIMD vectorization (no explicit use of `#[cfg(target_arch = "x86_64")]` with SSE/AVX intrinsics, or use of the `wide` crate)
- The fast path (same resolution) uses a simple loop that modern compilers may auto-vectorize, but the bilinear path almost certainly won't auto-vectorize due to the inner loop complexity and float operations
- The `clamp(0.0, 255.0)` is redundant if source is properly clamped BGRA (0-255)

**Recommendation:**
- Use integer bilinear (fixed-point arithmetic) for the interpolation: multiply by 256 or 65536, use integer shift for division
- Add SSE2/AVX2 codepaths for the pixel format conversion using `#[cfg(target_feature = "sse2")]` or the `safe_arch` crate
- Use the `image` crate's built-in resize with a faster algorithm (NearestNeighbor is sufficient for recording, faster than Bilinear)
- Or: accept CPU-readback BGRA without scaling (use native resolution) to take the fast path always

---

### F-12: `drain_encoder_packets()` NAL unit parsing runs on every frame (debug logging)

| Field | Value |
|-------|-------|
| **Location** | `ffmpeg/software.rs:42-80` — `drain_encoder_packets()` |
| **Severity | Low |
| **Category | Debug overhead in release builds |
| **Impact** | Marginal in release; `tracing::debug!()` is disabled by default |
** |
| **Impact** | Marginal — tracing debug is disabled by default in release builds |

**Description:**
Every frame, `drain_encoder_packets()` parses the HEVC NAL unit type from encoded packets for debug logging. While the actual logging is gated on `tracing::debug!()` (disabled by default in release builds), the NAL header parsing (`data[4] >> 1) & 0x3f`) still runs unconditionally.

**Code:**
```rust
// software.rs:60-68 (in drain_encoder_packets)
let hevc_nal: Option<u8> = if data.len() >= 5 && data[0..4] == [0x00, 0x00, 0x00, 0x01] {
    Some((data[4] >> 1) & 0x3f)
} else if data.len() >= 4 && data[0..3] == [0x00, 0x00, 0x01] {
    Some((data[3] >> 1) & 0x3f)
} else {
    None
};
// then used only for debug logging...
let nal_name = match hevc_nal { ... };
```

**Why this is suboptimal:**
- NAL parsing runs for **every** packet, every frame, even though the result is only used in debug logging
- The parsing involves byte comparisons and bit shifts — very cheap but still unnecessary in the hot path
- The `nal_name` match (8 arms) is also always evaluated

**Recommendation:**
- Gate the NAL parsing and match behind `if tracing::enabled!(tracing::Level::DEBUG)` to skip completely when debug logging is disabled
- Or move packet logging to a separate conditional block

---

### F-13: Missing `use_cpu_readback` optimization for GPU frame pipeline

| Field | Value |
|-------|-------|
| **Location** | `encoder_mod/types.rs` — `ResolvedEncoderConfig::use_cpu_readback` |
| **Severity** | Low-Medium |
| **Category** | Configuration / dead paths |
| **Impact** | Setting exists but hardware path always prefers GPU transport |

**Description:**
`ResolvedEncoderConfig` has a `use_cpu_readback: bool` field (defaulting to `true` in `EncoderConfig::new()`). However, in the hardware encoder path (`FfmpegEncoder::encode_frame`), the presence of a `d3d11` frame with matching format always takes the GPU path regardless of this flag. The flag is documented as "Phase 1 fallback" but the fallback logic isn't fully implemented — the encoder either initializes for GPU transport or CPU path based on the frame's contents, not the config flag.

**Code:**
```rust
// mod.rs:115-122
let can_use_gpu = match gpu_frame {
    Some(gf) => self.supports_gpu_frames() && self.gpu_frame_matches_encoder(gf),
    None => false,
};
// use_cpu_readback is never consulted here
```

**Why this is suboptimal:**
- The `use_cpu_readback` flag is dead configuration — it doesn't influence behavior in the current code
- If a user explicitly set `use_cpu_readback = true` expecting CPU fallback (e.g., for debugging), it would be ignored
- The flag adds complexity and potential confusion

**Recommendation:**
- Either implement the flag: check `self.config.use_cpu_readback` before deciding to use GPU transport
- Or remove the flag entirely and update the config schema/documentation
- If keeping it: use it to force BGRA readback even when GPU textures are available (useful for debugging/compatibility)

---

## Scoring

| Area | Score | Rationale |
|------|-------|-----------|
| **Hardware encoder init overhead** | 6/10 | Probe does 3 full encoder opens; D3D11 context creation redundant on same device |
| **Frame submission path** | 8/10 | Crossbeam channel with batching and burst handling is well-designed; double-drain is minor |
| **Encoder selection flow** | 7/10 | Clean fallback chain; probe-heavy but cached per session; AMF keyframe fix is good |
| **GPU texture → encoder** | 8/10 | Shared device path avoids staging to CPU; NVENC/AMF reusable frame; QSV alloc+map+free is the weak link |
| **Software encoder path** | 6/10 | libx265 ultrafast heavy for CPU; floating-point bilinear scaling unoptimized; no thread affinity |
| **Thread management** | 5/10 | BELOW_NORMAL priority is good; no affinity; worker threads unnamed; no E-core guidance |
| **Packet return pipeline** | 8/10 | Batch flush, bounded channels, `Bytes` zero-copy; `Instant::now()` per flush is minor nit |
| **Overall** | **7.2/10** | Solid foundation; QSV per-frame alloc is the biggest actionable; thread affinities and probe optimization are medium-effort wins |

---

## Summary of Recommendations (by priority)

1. **HIGH** — QSV: reuse mapped frame instead of alloc/map/free per frame
2. **HIGH** — Add CPU affinity for encoder threads (E-cores on hybrid, or dedicated core)
3. **MEDIUM** — Add lightweight GPU vendor pre-check before encoder probe (skip non-existent vendors)
4. **MEDIUM** — Reduce `drain_ready_packets()` calls: drain once after burst, not after every frame
5. **MEDIUM** — Cache D3D11 hardware context across encoder reinitializations
6. **MEDIUM** — Software encoder: use fixed-point arithmetic for bilinear scaling, add SIMD, or use NearestNeighbor
7. **LOW** — Gate NAL unit parsing behind `tracing::enabled!()` check
8. **LOW** — Reduce `pending_packet_timestamps` initial capacity
9. **LOW** — Name and prioritize software encoder worker threads
10. **LOW** — Implement or remove `use_cpu_readback` flag
