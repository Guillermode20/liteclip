# liteclip-core

Windows screen capture and replay buffer engine.

## Usage

```rust
use liteclip_core::{ReplayEngine, AppDirs};

let engine = ReplayEngine::new(AppDirs::from_app_slug("my-app"))?;
engine.start_recording().await?;
// ... later ...
engine.save_clip(duration, output_path).await?;
```

## Setup

1. Link against FFmpeg dev libraries (matching `ffmpeg-next` version)
2. Place FFmpeg DLLs next to your executable
3. Call `encode::init_ffmpeg()` before starting

## Requirements

- Windows 10+
- FFmpeg 6.0+ shared libraries

See [docs.rs](https://docs.rs/liteclip-core) for API details and examples.
