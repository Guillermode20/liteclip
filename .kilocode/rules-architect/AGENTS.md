# AGENTS.md - Architect Mode

## Architectural Constraints

### Threading Architecture
- **Never block the tokio runtime**: All sync operations (encoding, muxing) spawn_blocking or use dedicated threads
- **Single encoder thread**: One encoder instance per recording session, joined before stop returns
- **Channel backpressure**: Encoder channel bounded(4) - capture will block if encoder can't keep up
- **Message pump isolation**: Win32 message loop runs in dedicated thread with hidden HWND, not in tokio

### Memory Architecture
- **Zero-copy where possible**: Bytes crate for reference-counted packet data
- **Snapshot safety**: Buffer snapshots via cheap Arc clone, no data copying
- **Budget enforcement**: Hard memory limit with eviction, not unbounded growth
- **RwLock choice**: parking_lot::RwLock chosen over std::sync for performance

### Event Flow Architecture
- Hotkey events: `Win32 WM_HOTKEY` → crossbeam channel → tokio mpsc → async handler
- Frame flow: Capture thread → crossbeam channel → Encoder thread → Ring Buffer
- No direct coupling: AppState owns buffer, encoder pushes to it via SharedReplayBuffer handle

### Windows-Only Design
- **No cross-platform abstractions**: Direct Win32 API usage throughout
- **Windows crate**: Modern windows-rs bindings, not raw FFI
- **Unsafe blocks**: All Win32 calls are unsafe, minimized surface area
- **Build dependencies**: Requires Windows SDK, links to d3d11/dxgi/user32/gdi32

### Feature Flag Architecture
- **FFmpeg optional**: Full functionality without FFmpeg in Phase 1, muxer stubbed
- **Conditional compilation**: #[cfg(feature = "ffmpeg")] guards all FFmpeg-dependent code
- **Graceful degradation**: Warning at runtime if FFmpeg not enabled, not a crash

### Config Architecture
- **TOML format**: Human-readable, serde-based serialization
- **Auto-creation**: Default config generated if missing on first run
- **Platform paths**: Uses dirs crate for proper Windows path resolution
- **Live reload not implemented**: Config loaded once at startup

### Pipeline Design
- **Phase 1 (MVP)**: Video-only, CPU readback path, software encoding fallback
- **Phase 2**: WASAPI audio capture, AAC encoding, A/V sync via QPC timestamps
- **Phase 3**: Zero-copy GPU path, hardware encoding priority, GUI with iced
- **Phase 4**: Advanced features - H.265/AV1, resolution scaling, clip tagging

### Extension Points
- New capture backend: Implement CaptureBackend trait in src/capture/
- New encoder: Add to src/encode/ with Encoder trait
- New hotkey: Update both hotkeys.rs and msg_loop.rs with matching IDs
- New codec: Add variant to config::Codec, update ffmpeg_codec_name()
