# AGENTS.md - Ask Mode

## Codebase Documentation Context

### Project Structure Clarification
- `src/capture/` - DXGI Desktop Duplication capture (not audio/mic yet)
- `src/encode/` - Video encoding with hardware acceleration (NVENC/AMF/QSV)
- `src/buffer/` - In-memory ring buffer for encoded packets (not raw frames)
- `src/clip/` - MP4 muxing and clip saving (stubbed without FFmpeg feature)
- `src/platform/` - Win32 message loop and hotkey handling (Windows-only)
- `src/d3d.rs` - D3D11 helper types (mostly stubbed for Phase 1)

### Key Implementation Files
- `src/app.rs` - Main AppState with recording lifecycle management
- `src/capture/dxgi.rs` - DXGI Desktop Duplication implementation
- `src/encode/mod.rs` - Encoder thread spawning and packet handling
- `src/buffer/ring.rs` - Thread-safe ring buffer with parking_lot RwLock
- `src/platform/msg_loop.rs` - Hidden HWND message pump
- `src/platform/hotkeys.rs` - Global hotkey registration
- `src/clip/muxer.rs` - MP4 muxing with optional FFmpeg

### Async/Sync Boundaries
- Main loop is async tokio (handles events, clip saving)
- Platform thread is sync Win32 (hotkeys, message pump)
- Capture thread is sync (DXGI frame acquisition)
- Encoder thread is sync (receives frames, pushes to buffer)
- Crossbeam channels bridge sync/async: `event_rx` → `tokio::mpsc::channel`

### Windows-Specific Dependencies
- `windows` crate - Win32 API bindings
- Requires Windows SDK installed
- Links to `d3d11`, `dxgi`, `dxguid`, `user32`, `gdi32`

### Phase Status
- Phase 1 (MVP): Capture + encode + buffer + hotkeys + CLI
- Phase 2 (Audio): WASAPI loopback + microphone + AAC encoding
- Phase 3 (GUI): iced GUI + system tray + overlay

### Rewrite Plan
- See `rewrite-plan.md` for full architecture specification
- Document is authoritative reference for planned features
- Code may not fully match plan yet (work in progress)
