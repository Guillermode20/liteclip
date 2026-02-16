//! GUI module for LiteClip Replay settings
//!
//! Provides an egui-based settings window for configuring the application.

mod app;

pub use app::{run_settings_window, run_settings_window_async, GuiResult, SettingsApp};
