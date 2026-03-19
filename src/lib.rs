//! LiteClip Replay — desktop application library facade.
//!
//! Re-exports the recording engine from [`liteclip_core`] so existing paths such as
//! `liteclip_replay::app::AppState` remain stable, and adds shell modules for the
//! full product (tray, hotkeys, settings, gallery, game detection).
//!
//! For embedding only the engine in another binary, depend on **`liteclip-core`** directly.

pub use liteclip_core::{app, buffer, capture, config, encode, hotkey_parse, media, output};

pub mod detection;
pub mod gui;
pub mod platform;
