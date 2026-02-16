# Architecture Rules

## Critical Implementation Details

### Hardware Encoders
- Encoder selection: `encode/mod.rs`
- FFmpeg command building: `encode/hw_encoder.rs`
- **CRITICAL for h264_amf**: Must use `-bf 0` (disable B-frames) or output is unplayable

### Frame Data Flow
- `CapturedFrame` contains BOTH D3D11 texture handle AND CPU BGRA bytes
- CPU readback happens unconditionally (Phase 1 limitation)
- This causes unnecessary memory copies when using hardware encoding

### Ring Buffer
- Eviction based on **memory bytes**, not duration
- `duration` field is informational only
- Keyframe indices rebuilt O(N) on eviction - can stall under memory pressure

### Error Handling
- Many errors log with `warn!`/`error!` and continue silently
- Encoder thread may die without propagating error
- Always check thread join results

### Configuration
- Stored at `%APPDATA%/liteclip-replay/liteclip-replay.toml`
- `use_native_resolution = true` ignores resolution config
- Actual resolution comes from first captured frame
