//! Configuration Management
//!
//! This module provides application configuration types and persistence.
//!
//! # Configuration Location
//!
//! Configuration is stored at `%APPDATA%\liteclip-replay\config.toml`.
//!
//! # Configuration Sections
//!
//! - **General**: Replay duration, auto-start, notifications
//! - **Video**: Framerate, bitrate, encoder, codec, resolution
//! - **Audio**: System/mic capture, volume levels
//! - **Hotkeys**: Key bindings for save, toggle, screenshot
//! - **Advanced**: GPU selection, CPU readback, overlay
//!
//! # Key Types
//!
//! - [`Config`] - Main configuration struct
//! - [`GeneralConfig`] - General application settings
//! - [`VideoConfig`] - Video encoding settings
//! - [`AudioConfig`] - Audio capture settings
//! - [`HotkeyConfig`] - Hotkey bindings
//! - [`AdvancedConfig`] - Advanced tuning options
//!
//! # Example
//!
//! ```no_run
//! use liteclip_replay::config::Config;
//!
//! // Load configuration (or use defaults)
//! let config = Config::default();
//!
//! // Modify settings
//! let mut config = config;
//! config.general.replay_duration_secs = 120;
//! config.video.bitrate_mbps = 15;
//!
//! // Save configuration
//! config.save_sync().unwrap();
//! ```

pub mod config_mod;

pub use config_mod::*;
