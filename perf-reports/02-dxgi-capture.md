# DXGI Capture Pipeline — Performance Analysis

## Summary

The DXGI capture pipeline is a GPU-accelerated screen capture engine using `IDXGIOutputDuplication` with two GPU frame transport modes: BGRA (zero-copy via shared textures + fences) and NV12 (via D3D11 Video Processor + shared textures + fences). The pipeline operates on a dedicated thread with adaptive frame pacing, backpressure-driven FPS scaling, and static-scene low-power mode at 5 FPS. Overall architecture is sound with GPU-side fence synchronization eliminating CPU stalls for both NV12 and BGRA paths. Key concerns are texture pool blocking under encoder pressure, unused backpressure tracking infrastructure, aggressive input view cache eviction, and sleep-based pacing that adds scene-change detection latency.

---

## Hot Paths

### Hot Path 1: AcquireNextFrame → BGRA CopyResource → fence signal → send to encoder
- **Invocation**: Every frame when encoder is keeping up and BGRA format is selected.
- **Cost profile**: Kernel transition (AcquireNextFrame), GPU copy (CopyResource), GPU fence signal.
- **CPU cost**: Low. No staging copies, no CPU readback. QPC timestamp call only.
- **GPU cost**: `CopyResource` copies the desktop backbuffer to a pooled shared texture.

### Hot Path 2: AcquireNextFrame → BGRA → VideoProcessorBlt → NV12 → fence signal → send to encoder
- **Invocation**: Every frame when NV12 conversion is available and active.
- **Cost profile**: Same as above + `VideoProcessorBlt` for format conversion.
- **CPU cost**: Low. Conversion is entirely GPU-side.
- **GPU cost**: `VideoProcessorBlt` bandwidth: ~2× the desktop resolution (BGRA read + NV12 write).

### Hot Path 3: AcquireNextFrame → ReleaseFrame (drop) → backpressure skip
- **Invocation**: Whenever `drop_before_process` is true (queue full or adaptive divisor active).
- **Cost profile**: Minimal — AcquireNextFrame + ReleaseFrame with no texture processing.
- **CPU cost**: Very low. Two kernel calls, no GPU work.

### Hot Path 4: Timeout → duplicate last frame
- **Invocation**: When `AcquireNextFrame` returns `WAIT_TIMEOUT` (no new desktop content).
- **Cost profile**: Reuses last captured frame (Arc clone or Bytes clone) and QPC timestamp.
- **CPU cost**: Very low. No GPU work, no kernel call for QPC (cached timestamp).

---

## Findings

### F1: Texture pool exhaustion blocks capture loop (HIGH)

| Field | Value |
|-------|-------|
| **Location** | `texture.rs:96-103` (BGRA), `texture.rs:128-135` (NV12) |
| **Severity** | **High** |
| **Description** | When both the available list is empty AND the pool is at capacity, `acquire_bgra_pool_item` and `acquire_nv12_pool_item` call `recv_timeout(Duration::from_millis(100))`, blocking the capture thread for up to 100ms. During this block, no frames can be acquired from DXGI, causing the encoder to stall. |
| **Code** | `match pool.return_rx.recv_timeout(Duration::from_millis(100)) { Ok(item) => return Ok(item), Err(_) => bail!(...) }` |
| **Why** | Pool capacity is 12 textures per format. If the encoder holds more than 12 frames simultaneously (e.g., due to a GPU encoding stall, or because the encoder pipeline has 12+ in-flight frames), the 13th allocation blocks. The 100ms blocking receive is a synchronous stall that prevents the capture loop from servicing `AcquireNextFrame`. |
| **Recommendation** | Replace the blocking `recv_timeout` with an immediate bail or a `try_recv` that falls back to dropping the current frame instead of blocking. If no pooled texture is available, the frame should be dropped (ReleaseFrame immediately) rather than blocking the capture loop. Alternatively, use a higher pool capacity (e.g., 24) or dynamic pool expansion up to a larger limit before blocking. |

### F2: `queued_frames` counter is unused dead code (LOW)

| Field | Value |
|-------|-------|
| **Location** | `backpressure.rs:20-22` |
| **Severity** | **Low** |
| **Description** | `BackpressureState.queued_frames` and `max_queued_frames` are defined and initialized but never incremented, decremented, or read anywhere in the codebase. `max_queued_frames` defaults to 8 but has no effect. |
| **Code** | `pub queued_frames: AtomicU32,` — defined, never written or read |
| **Why** | Dead code from a previous iteration. The capture loop uses `frame_tx.len()` directly for queue depth monitoring instead of relying on this counter. The counter would need atomic increment/decrement across capture→encoder boundaries, which `crossbeam::channel::len()` provides for free. |
| **Recommendation** | Remove `queued_frames` and `max_queued_frames` from `BackpressureState` to eliminate dead code. If frame counting is needed for metrics, use `frame_tx.len()` which is already used throughout the capture loop. |

### F3: `report_frame_result` / EMA drop rate is defined but never called (MEDIUM)

| Field | Value |
|-------|-------|
| **Location** | `backpressure.rs:49-59` |
| **Severity** | **Medium** |
| **Description** | `BackpressureState::report_frame_result()` computes an EMA of the drop rate and sets `encoder_overloaded` based on a 10% threshold. However, this method is never called from the capture loop. Instead, `set_encoder_overloaded(true/false)` is called directly based on `TrySendError::Full` vs success, bypassing the smoothing logic. |
| **Code** | `report_frame_result` has `was_dropped` parameter and EMA logic — never invoked |
| **Why** | The capture loop at `capture.rs:329-346` calls `backpressure.set_encoder_overloaded(true)` immediately on any channel-full send failure, and `set_encoder_overloaded(false)` on any success. This causes the overload flag to oscillate rapidly: a single timeout-frame send success clears overload, then the next real frame fill fails and sets it again. The EMA was designed to prevent this oscillation but isn't wired up. |
| **Recommendation** | Call `backpressure.report_frame_result(was_dropped)` in every iteration of the capture loop's frame-processing paths (success, dropped, timeout). Remove the direct `set_encoder_overloaded` calls and let the EMA govern the flag. This prevents the overload flag from toggling on every single frame during marginal encoder load. |

### F4: Input view cache fully cleared every 64 fences (MEDIUM)

| Field | Value |
|-------|-------|
| **Location** | `texture.rs:239-242` |
| **Severity** | **Medium** |
| **Description** | The `input_view_cache` (which caches `ID3D11VideoProcessorInputView` objects by raw texture pointer) is fully cleared every 64 NV12 BLT operations. This destroys all cached views at once, causing 64+ views to be recreated in subsequent frames. |
| **Code** | `if state.nv12_fence_value % 64 == 0 && !state.input_view_cache.is_empty() { state.input_view_cache.clear(); state.input_view_cache_fifo.clear(); }` |
| **Why** | The comment explains this is to prevent stale entries when pooled textures are recycled at the same address. However, a full clear every 64 frames causes a burst of `CreateVideoProcessorInputView` calls, each taking a nontrivial GPU driver call. Since the cache is already FIFO-limited to 16 entries (with per-entry eviction), the periodic full clear is redundant for steady-state capture. |
| **Recommendation** | Remove the full periodic clear. The FIFO eviction (max 16 entries) already bounds cache growth and handles pool recycling naturally — recently used textures stay cached, and recycled textures at the same address will have their old views reused correctly (the view is still valid if the texture's underlying allocation hasn't changed). If stale-address concerns are real, evict only entries matching the current `nv12_fence_value % 64 == 0` cycle for textures created prior to the clear point, rather than clearing everything. |

### F5: Sleep-based low-power mode adds ~200ms scene-change latency (MEDIUM)

| Field | Value |
|-------|-------|
| **Location** | `capture.rs:247-250` |
| **Severity** | **Medium** |
| **Description** | When `is_low_power_mode` is active (static scene, target FPS=5), the capture loop sleeps for `low_power_frame_period` (200ms) before calling `AcquireNextFrame` with a 1ms peek timeout. This adds up to 200ms of latency between scene content changing and detection. |
| **Code** | `std::thread::sleep(low_power_frame_period);` — 200ms sleep before peek |
| **Why** | The sleep is intentional to avoid busy-looping during static scenes. However, 200ms of sleep before even checking for new content means that after a scene change (e.g., user starts typing or a game frame updates), it takes 200ms before the system recognizes the change and switches back to full-rate capture. This is noticeable in interactive content like gaming or real-time video. |
| **Recommendation** | Reduce the sleep duration or use `AcquireNextFrame` with a longer timeout (e.g., 200ms) instead of sleep+peek. `AcquireNextFrame(timeout_ms=200)` blocks in the kernel until either a new frame arrives or the timeout expires — this gives zero-latency response to scene changes while maintaining low CPU usage during static scenes. If the kernel-wait approach works (test on Win10/11), it eliminates the polling entirely. |

### F6: Frame pacing schedule reset is sensitive to frame_period drift (LOW)

| Field | Value |
|-------|-------|
| **Location** | `capture.rs:381-383` |
| **Severity** | **Low** |
| **Description** | The frame pacing accumulator (`next_frame_time += frame_period`) can drift if `next_frame_time` falls behind by more than one frame_period. The reset condition checks `> next_frame_time + frame_period`, meaning a 1.5× frame_period drift is tolerated before reset. |
| **Code** | `if std::time::Instant::now() > next_frame_time + frame_period { next_frame_time = std::time::Instant::now() + frame_period; }` |
| **Why** | Frame pacing uses `Instant::now()` for sleep deadlines. Thread scheduling delays, reinit pauses, or adaptive FPS changes can cause the deadline to fall behind. The reset threshold of `frame_period` means that at 60 FPS (~16.7ms), drift up to ~33ms is tolerated before reset. This is fine for screen recording but could cause bursty frame delivery after reinit. |
| **Recommendation** | Consider resetting to `now` (not `now + frame_period`) on severe drift to avoid sending one frame immediately after a reinit delay, which would cause a visually duplicate or near-zero-delta frame. Otherwise, current behavior is acceptable. |

### F7: Adaptive FPS recovery takes 6+ seconds (LOW)

| Field | Value |
|-------|-------|
| **Location** | `capture.rs:337-375` |
| **Severity** | **Low** |
| **Description** | When the encoder recovers from overload, the `fps_divisor` is decremented by 1 every 2-second adjustment cycle (requires `low_streak_threshold=3` consecutive low-pressure samples). If `fps_divisor` was ramped up to 3 (effective FPS = 60/4 = 15 on a 60 FPS target), recovery to divisor 0 (full 60 FPS) takes 3 cycles × 2 seconds = 6 seconds. |
| **Code** | `if pressure_low_streak >= decision.low_streak_threshold && fps_divisor > 0 { fps_divisor -= 1; ... }` — decrements by 1 per 2s cycle |
| **Why** | The 3-streak requirement prevents premature recovery from a transient encoder stall, but when the encoder genuinely recovers (e.g., after a scene complexity decrease), 6 seconds of suboptimal FPS is noticeable in recordings. The 2-second cycle also means a single overload spike causes 2 seconds of suppressed FPS before recovery begins. |
| **Recommendation** | Consider a faster recovery path: after `fps_divisor` has remained stable for several cycles without any drops, reduce divisor by 1 every 1 second instead of 2. Or use an asymmetric response: aggressive increase (every high-pressure sample), gradual decrease (every 3 low-pressure samples). The current 2:1 ratio of increase-to-decrease streaks is reasonable but the 2-second period makes recovery slow. |

### F8: Thread priority NORMAL may cause capture starvation under game load (MEDIUM)

| Field | Value |
|-------|-------|
| **Location** | `capture.rs:420-431` |
| **Severity** | **Medium** |
| **Description** | The capture thread is explicitly set to `THREAD_PRIORITY_NORMAL`. The comment says this avoids competing with the game thread (usually HIGH or TIME_CRITICAL). However, `AcquireNextFrame` is a blocking kernel call that does not consume CPU while waiting. Priority only affects scheduled CPU time, and the capture thread's CPU work is minimal (Bytes clone, channel send). Setting it to BELOW_NORMAL would be safer to ensure the game thread is never preempted by the capture thread's wake-up. |
| **Code** | `SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_NORMAL)` |
| **Why** | While the capture thread does relatively little CPU work per frame, at 60 FPS it wakes up 60 times/sec to process the AcquireNextFrame result. If the game is CPU-bound and the capture thread's wake-up preempts the game thread on the same core, frame time jitter increases. Using BELOW_NORMAL ensures the game always gets priority. |
| **Recommendation** | Change to `THREAD_PRIORITY_BELOW_NORMAL` or `THREAD_PRIORITY_LOWEST`. The overhead of running at NORMAL vs BELOW_NORMAL for a thread that mostly sleeps is negligible for capture latency (the GPU work is unaffected by thread priority). Only the channel send and metrics update run at this priority, and those are sub-millisecond operations. |

### F9: BGRA path uses CopyResource without explicit GPU fence wait in encoder (MEDIUM)

| Field | Value |
|-------|-------|
| **Location** | `texture.rs:282-300` (BGRA copy + fence signal) |
| **Severity** | **Medium** |
| **Description** | The BGRA capture path calls `CopyResource` followed by `Signal` on the BGRA fence. The encoder must wait on this fence before reading the shared texture. If the fence creation failed earlier (falling back to `Flush()`), synchronization is purely CPU-side: `Flush()` guarantees the copy is submitted, but does not guarantee completion before the encoder reads the texture. On systems where `ID3D11Device5` is unavailable (pre-Win10 or driver issues), this path lacks proper GPU synchronization. |
| **Code** | `CopyResource(...)` then `Signal(fence, value)` if fence available, else `Flush()` with warning |
| **Why** | `Flush()` submits the command to the GPU but returns immediately. The encoder thread could start reading the shared texture before the GPU copy completes, resulting in a partial frame (tearing) or stale data. On most hardware, the copy completes within a few microseconds, but there is no guaranteed ordering without the fence. |
| **Recommendation** | For the fallback path without fences, consider using `D3D11_QUERY_EVENT` on the encoder side to wait for completion, or add a CPU-based spin wait with a small timeout. Better yet, document this as a known limitation and recommend Win10+ (where ID3D11Device5 is always available on modern GPUs). |

### F10: Duplicate frame reuses stale `last_frame` when `last_frame` is `None` but NV12 conversion was skipped (LOW)

| Field | Value |
|-------|-------|
| **Location** | `capture.rs:269-270` |
| **Severity** | **Low** |
| **Description** | In the timeout path, `let Some(ref last) = last_frame else { continue; }` skips duplicate frame generation if no frame has been captured yet. However, the NV12 conversion retry backoff can cause multiple consecutive errors, setting `last_frame` to `None` if the first frame capture attempt failed. This means during NV12 failure recovery, duplicate frame generation is suppressed until a successful capture. |
| **Code** | `let Some(ref last) = last_frame else { continue; }` — skips if no valid last frame |
| **Why** | This is a minor behavioral issue: during the backoff period after NV12 failure, the capture loop acquires frames successfully but fails to convert them. If the conversion failure is transient and a prior successful BGRA frame was captured, `last_frame` contains a valid BGRA frame. However, the failure path in `capture_gpu_frame` returns an error before updating `last_frame`, so the last successful frame is preserved. This is actually correct behavior — just non-obvious. |
| **Recommendation** | No change needed. Consider adding a comment clarifying that `last_frame` is only updated on successful capture, so the guard correctly prevents stale data. |

### F11: Reinit discards in-flight pooled textures (MEDIUM)

| Field | Value |
|-------|-------|
| **Location** | `capture.rs:350-359` |
| **Severity** | **Medium** |
| **Description** | When a DXGI access-lost or error triggers reinitialization, the entire `DxgiCaptureState` is replaced (`state = new_state`). The old state's `Drop` runs, closing fence handles. However, any pooled textures still in-flight (held by the encoder via `Arc<D3d11Frame>`) have their `return_tx` pointing to the old pool's channel, which is dropped alongside the old state. When the encoder eventually drops these frames and tries to return the texture, the send goes to a closed channel. |
| **Code** | `match reinit_result { Ok(new_state) => { state = new_state; ... } ... }` — old state dropped |
| **Why** | The `crossbeam::channel::Sender::send` on a closed channel returns an error which is silently ignored (`let _ = tx.send(item)` in `D3d11Frame::Drop`). The pooled D3D11 textures held by the encoder are silently leaked — their COM resources are not leaked (the texture's COM ref count keeps it alive), but the pool loses track of them, so the pool's texture count becomes inconsistent with actual allocation. |
| **Recommendation** | Before replacing state during reinit, drain the old return channels to collect any in-flight textures and properly release them. Alternatively, use an `Arc<Mutex<Vec<...>>>` shared pool that survives reinit, so that in-flight textures can be returned to the new pool instance. At minimum, log a warning when old pool channels are dropped with outstanding textures. |

### F12: NV12 pool output views tied to original enumerator but survive reinit (LOW)

| Field | Value |
|-------|-------|
| **Location** | `texture.rs:62-80` |
| **Severity** | **Low** |
| **Description** | Each NV12 pooled texture has an `ID3D11VideoProcessorOutputView` created at pool creation time. These views are created against the original `video_processor_enumerator`. If the state is reinitialized (new enumerator), these cached output views become invalid for the new enumerator, even if the resolution is the same. |
| **Code** | `video_device.CreateVideoProcessorOutputView(&texture, enumerator, ...)` — views created with original enumerator |
| **Why** | On reinit, the old enumerator is dropped and a new one is created. Output views from the old enumerator reference the old enumerator's state. Using them with a new video processor (created from the new enumerator) could cause `E_INVALIDARG` or undefined behavior. Since pool items are transferred via `Arc<D3d11Frame>`, stale output views in the encoder's hands could be used after reinit. |
| **Recommendation** | During reinit, explicitly release all pooled textures and their views. Current behavior relies on the old state's Drop cleaning up, but in-flight `Arc<D3d11Frame>` copies held by the encoder bypass this. Add a check in the encoder path to detect stale view enumerators (e.g., version counter). |

---

## Scoring

| Dimension | Score (1-10) | Notes |
|-----------|--------------|-------|
| **GPU-CPU sync efficiency** | 8 | Fence-based GPU-side ordering is excellent. Flush fallback is a known weak point but only triggers on pre-Win10. |
| **Texture pool design** | 5 | Pool size of 12 constrains pipeline depth; blocking on exhaustion is a critical issue (F1). No shrink mechanism on resolution change. |
| **Video processing** | 7 | VideoProcessorBlt on GPU is efficient. Input view cache has periodic full-clearing issue (F4). |
| **Multithreaded safety** | 7 | ID3D11Multithread enabled once. No lock contention. COM interfaces are thread-safe. Pool recycling uses channels. |
| **Frame pacing** | 6 | Sleep-based pacing is simple but low-power mode adds 200ms latency (F5). Schedule reset is reasonable. |
| **Backpressure integration** | 4 | Unused EMA/queued_frames counters (F2, F3). Adaptive recovery is slow (F7). Oversized channels compensate for weak backpressure. |
| **Channel architecture** | 8 | bounded(32) frame channel is generous. Pool return channels at 2× capacity. Channel operations are non-blocking (except F1). |
| **Memory allocation** | 9 | Minimal CPU allocations. GPU textures are pooled and recycled. `Bytes::new()` for GPU frame bgra avoids wasting memory. Timestamp caching avoids QPC on duplicates. |
| **Thread priority** | 4 | NORMAL is too high for a background capture thread (F8). Should be BELOW_NORMAL to avoid game interference. |
| **Error recovery** | 5 | Reinit leaks in-flight pooled textures (F11). Exponential backoff is good. No way to discard stale encoder-held views after reinit (F12). |

### Overall Score: **6.2 / 10**

Strengths: GPU-side fence synchronization eliminates CPU stalls in steady state. Efficient zero-copy GPU frame transport. No CPU-side frame copies. Reuse of timestamps avoids unnecessary QPC kernel calls.

Weaknesses: Texture pool blocking on exhaustion can stall the capture pipeline. Backpressure infrastructure has dead/inactive code paths. Low-power mode adds noticeable scene-change detection latency. Thread priority should be lowered. Reinit doesn't handle in-flight pooled textures.
