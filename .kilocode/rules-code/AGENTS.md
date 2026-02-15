# AGENTS.md - Code Mode

This file provides coding guidance specific to this repository.

## Critical Implementation Details

### Hotkey ID Constants Must Stay In Sync
- Hotkey IDs are defined in BOTH `src/platform/hotkeys.rs` AND `src/platform/msg_loop.rs`
- Adding a new hotkey requires updating BOTH files
- IDs: `1000` (save), `1001` (toggle), `1002` (screenshot), `1003` (gallery)

### Encoder Handle Pattern
- Encoder thread must be joined synchronously before returning from `stop_recording()`
- Use `handle.thread.join()` (not just dropping) to ensure all packets flush
- This is NOT a bug - it's required for proper MP4 finalization

### Ring Buffer Memory Management
- Uses `Bytes` crate for reference-counted data (cheap clone = ref count bump)
- Snapshotting buffer for clips does NOT copy data - only clones Arc<Bytes>
- Buffer uses `parking_lot::RwLock` (not std::sync) for better performance

### FFmpeg Feature Guards
- All FFmpeg code must use `#[cfg(feature = "ffmpeg")]` conditional compilation
- Without feature: muxer returns stub success, clips won't play
- Run with `--features ffmpeg` for playable output

### Config Path Resolution
- Config stored at `%APPDATA%/liteclip-replay/liteclip-replay.toml`
- Use `dirs::data_dir()` (not `dirs::config_dir()`) for cross-Windows compatibility
- File auto-created with defaults if missing on first run

### Error Handling
- Always use `anyhow::Result` and `anyhow::Context`
- Use `?` operator with `.context("message")` for rich error messages
- Never use `Result<T, Box<dyn Error>>` - use anyhow instead

### Win32 API Safety
- All Win32 calls are `unsafe` blocks
- Always wrap in `unsafe { ... }` with context comment
- Use `windows` crate types (e.g., `HWND`, `HOT_KEY_MODIFIERS`) not raw integers

### Frame Pipeline
- Capture → `crossbeam::channel` → Encoder → Ring Buffer
- Channel backpressure: encoder channel bounded(4) to prevent overflow
- Dropping capture signals encoder to stop via channel close
