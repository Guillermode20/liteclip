//! LiteClip core — Windows screen capture, encoding, replay ring buffer, and muxing.
//!
//! This crate is the engine behind LiteClip Replay. Host applications (tray apps, games,
//! overlays) can depend on it to run continuous capture into a retroactive buffer and
//! save clips on demand.
//!
//! # Supported embedder path
//!
//! For a **background clipping engine**, depend on this crate and use:
//!
//! - [`ReplayEngine`] — lifecycle, config-backed state, and [`ReplayEngine::save_clip`].
//! - [`prelude`] — common imports (`ReplayEngine`, [`Config`], [`AppDirs`], [`CoreHost`], [`encode`], [`runtime`]).
//! - [`paths::AppDirs`] — isolate config and clips from the LiteClip Replay desktop layout.
//! - [`runtime`] — resolve or override the `ffmpeg` / `ffmpeg.exe` binary ([`runtime::FFMPEG_ENV`], [`runtime::set_ffmpeg_path`]).
//! - [`host::CoreHost`] — optional UI hooks ([`CoreHost::on_clip_saved`], [`CoreHost::on_pipeline_fatal`]).
//!
//! Call [`encode::init_ffmpeg`] when the `ffmpeg` feature is enabled **before** starting the pipeline.
//!
//! # Requirements
//!
//! - **Windows** (DXGI capture, WASAPI audio, D3D11). Other targets may compile in limited
//!   configurations but are **unsupported**; file issues only for `x86_64-pc-windows-msvc`.
//! - **FFmpeg** at runtime: the `ffmpeg-next` crate links to FFmpeg libraries; some paths also
//!   spawn an `ffmpeg` / `ffmpeg.exe` process. See [`runtime`] for how the binary is resolved
//!   ([`runtime::FFMPEG_ENV`], [`runtime::set_ffmpeg_path`], exe-relative search, and dev-only
//!   `ffmpeg_dev` heuristics when `debug_assertions` or the `dev-ffmpeg-paths` feature is on).
//!
//! # Tokio and async clip save
//!
//! [`ReplayEngine::save_clip`] is `async` and schedules work on the Tokio runtime (including
//! blocking offload). Use a multi-thread Tokio runtime (`tokio::runtime::Builder::new_multi_thread`)
//! in the host process, or wrap the engine in `spawn_blocking` patterns consistent with
//! [`AppState`](app::AppState) docs.
//!
//! # Application directories
//!
//! Use [`paths::AppDirs`] so your app does not share `%APPDATA%\liteclip-replay\` with the
//! desktop product. [`AppDirs::liteclip_replay`] matches the LiteClip Replay layout exactly.
//! [`Config::load_with_dirs`] / [`Config::save_to_dirs`] persist settings relative to that layout.
//!
//! # Data flow
//!
//! ```text
//! Capture → Encode → Buffer → Output (on save)
//! ```
//!
//! # Advanced / low-level modules
//!
//! These are public for power users and internal reuse; semver may move internals more than the
//! [`ReplayEngine`] facade:
//!
//! - [`app`], [`capture`], [`encode`], [`buffer`], [`output`], [`media`]
//!
//! # Feature flags
//!
//! - `ffmpeg` (default) — Pulls in optional `ffmpeg-next` for linked FFmpeg APIs, native MP4 muxing
//!   (`output::mp4`), and the `encode::ffmpeg` module. [`encode::init_ffmpeg`] is a no-op when this feature is off.
//!   With `default-features = false`, the crate still compiles for config-only / tooling use; **recording
//!   and clip export require `ffmpeg`.**
//!   The workspace root crate disables default features on `liteclip-core` and enables `ffmpeg` explicitly;
//!   standalone embedders typically use `default-features = true` or `features = ["ffmpeg"]`.
//! - `dev-ffmpeg-paths` — Include repo `ffmpeg_dev\...` search heuristics in **release** builds
//!   (debug builds already use them when `debug_assertions` are on).
//!
//! # Versioning
//!
//! This crate is versioned **independently** of the `liteclip-replay` binary. Breaking API changes
//! here use semver; bump the major version when removing or changing public types.
//!
//! # Modules
//!
//! - [`prelude`] — Small set of types most embedders need.
//! - [`app`] — [`AppState`](app::AppState), recording pipeline, [`ClipManager`](app::ClipManager).
//! - [`ReplayEngine`](crate::ReplayEngine) — thin facade over [`AppState`](app::AppState) and [`AppDirs`](paths::AppDirs).
//! - [`buffer`] — SPMC ring buffer for replay storage.
//! - [`capture`] — DXGI video and WASAPI audio capture.
//! - [`encode`] — Video encoding (NVENC, AMF, QSV, software) via FFmpeg.
//! - [`config`] — Configuration types and persistence.
//! - [`output`] — Muxing, thumbnails, clip export helpers.
//! - [`media`] — Shared frame types for capture and encode.
//! - [`paths`] — [`AppDirs`] for config / default clip folder layout.
//! - [`runtime`] — FFmpeg executable resolution and overrides.
//! - [`host`] — Optional [`CoreHost`] callbacks.
//! - [`hotkey_parse`] — Hotkey string parsing for your own global hotkey layer.
//!
//! # Example
//!
//! ```no_run
//! use liteclip_core::encode;
//! use liteclip_core::prelude::*;
//!
//! fn main() -> anyhow::Result<()> {
//!     encode::init_ffmpeg()?;
//!     let dirs = AppDirs::from_app_slug("my-app")?;
//!     let mut engine = ReplayEngine::with_default_config(dirs)?;
//!     engine.start_recording()?;
//!     engine.stop_recording()?;
//!     Ok(())
//! }
//! ```

pub mod app;
pub mod buffer;
pub mod capture;
pub mod config;
pub mod encode;
pub mod host;
pub mod hotkey_parse;
pub mod media;
pub mod output;
pub mod paths;
pub mod runtime;

mod engine;
pub use engine::ReplayEngine;

/// Commonly used items for embedders (see crate root docs).
///
/// For full engine control, import [`crate::app::AppState`] and submodules from [`crate`] directly.
pub mod prelude {
    pub use crate::app::{AppState, ClipManager};
    pub use crate::buffer::ReplayBuffer;
    pub use crate::config::Config;
    pub use crate::encode;
    pub use crate::host::CoreHost;
    pub use crate::paths::AppDirs;
    pub use crate::runtime::{self, set_ffmpeg_path, FFMPEG_ENV};
    pub use crate::ReplayEngine;
}
