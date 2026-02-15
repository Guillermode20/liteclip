# AGENTS.md - Debug Mode

## Debugging Guidelines

### Logging Configuration
- Uses `tracing` crate with `tracing_subscriber::fmt()`
- Logs go to stdout with timestamps and levels
- Use `RUST_LOG=debug` environment variable for verbose output
- In code: `info!`, `debug!`, `trace!`, `warn!`, `error!` macros

### Common Failure Points

#### DXGI Capture Failures
- `DXGI_ERROR_ACCESS_DENIED`: Another app is using Desktop Duplication (e.g., OBS)
- `DXGI_ERROR_ACCESS_LOST`: Display mode changed (resolution, refresh rate)
- Check adapter enumeration in `src/capture/dxgi.rs` - may need to try multiple GPUs

#### Hotkey Registration Failures
- Hotkeys fail silently if another app has registered them
- Check Windows Event Viewer for `RegisterHotKey` errors
- Hotkey IDs must be unique system-wide (use 1000+ range)

#### FFmpeg Muxer Issues
- Without `--features ffmpeg`, muxer is stubbed - clips won't be playable
- Check for `#[cfg(feature = "ffmpeg")]` guards when debugging muxer code
- `src/clip/muxer.rs` has separate implementations for with/without FFmpeg

#### Encoder Thread Panics
- If encoder panics, the `handle.thread.join()` will return `Err`
- Check capture-to-encoder channel - closing capture signals encoder stop
- Frame channel is bounded(4) - backpressure if encoder can't keep up

### Thread Debugging
- **Main thread**: Tokio async runtime, handles events
- **Platform thread**: Hidden HWND in `src/platform/msg_loop.rs`
- **Capture thread**: Spawned in `DxgiCapture::start()`
- **Encoder thread**: Spawned in `spawn_encoder_with_receiver()`

Use `std::thread::current().name()` or tracing spans to track thread context.

### Memory Debugging
- Ring buffer uses `Bytes` crate - check reference counts with `Arc::strong_count()`
- Memory budget: `max_memory_bytes` in config, evicts when exceeded
- Use `buffer.stats()` to get current memory usage

### Config Debugging
- Config path: `%APPDATA%/liteclip-replay/liteclip-replay.toml`
- Check file exists with `Config::config_path()`
- Auto-creates with defaults if missing
