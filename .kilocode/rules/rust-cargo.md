# Rust and Cargo Rules

## Build and Development

- Use `--features ffmpeg` flag for builds, tests, and clippy since this project requires FFmpeg
- Release builds use `lto = "fat"` and `panic = "abort"` - warn users about longer compile times
- Prefer `cargo check` over `cargo build` for faster feedback during development

## Code Style

- Follow existing import grouping: `std`, external crates, then `crate::`
- Use `anyhow::Result` for error handling
- Use `tracing` macros (`info!`, `warn!`, `error!`, `debug!`) for logging
- Prefer `Bytes` from `bytes` crate for reference-counted buffers
- Use `parking_lot::RwLock` instead of `std::sync::RwLock`

## Testing

- Run single tests with: `cargo test --features ffmpeg <test_name>`
- Run module-specific tests with: `cargo test --features ffmpeg <module>::`
- Most unit tests work without FFmpeg: `cargo test` (no features needed)
- Hardware encoder tests require FFmpeg and compatible GPU

## Windows-Specific Considerations

- This is a **Windows-only** application using Win32 APIs
- Use `windows` crate for Win32 API bindings
- Minimize unsafe blocks and document with `// SAFETY:` comments
- Prefer `windows` crate safe wrappers where possible
