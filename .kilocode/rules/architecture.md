# Architecture Rules

## Critical Implementation Details

### Hardware Encoders
- Encoder selection: `encode/mod.rs`
- FFmpeg command building: `encode/hw_encoder/types.rs`
- **CRITICAL for h264_amf**: Must use `-bf 0` (disable B-frames) or output is unplayable (Already implemented)

### Frame Data Flow
- Hardware Encoders bypass `DxgiCapture` completely and use FFmpeg `ddagrab` (hardware pull mode) for zero-copy GPU capture.
- `DxgiCapture` is used primarily for software encoding or CPU readback fallback.
- `CapturedFrame` contains CPU BGRA bytes via a reference-counted `Bytes` struct. Unconditional GPU texture holding was refactored out.

### Ring Buffer
- Eviction handles both `max_memory_bytes` and `duration` limits effectively.
- Keyframe tracking uses `VecDeque` instead of BTreeMap for `O(1)` amortized eviction, avoiding stalls under memory pressure.

### Error Handling
- The recording pipeline utilizes `EncoderHealthEvent` to propagate fatal errors via channels.
- `AppState::enforce_pipeline_health` correctly polls for crashes or thread deaths to trigger safe pipeline shutdowns.

### Configuration
- Stored at `%APPDATA%/liteclip-replay/liteclip-replay.toml`
- `use_native_resolution = true` overrides config `resolution`, inheriting the dynamic feed resolution properly.
