//! Graphical User Interface
//!
//! This module provides GUI components built with egui for settings,
//! clip gallery, and overlay notifications.
//!
//! # Components
//!
//! - **Settings GUI**: Configuration interface for video, audio, and hotkeys
//! - **Gallery GUI**: Browse and manage saved clips
//! - **Clip Saved Overlay**: Transient overlay showing clip save confirmation
//!
//! # Architecture
//!
//! GUI windows run in separate threads with their own egui/wgpu contexts.
//! Communication with the main application is via message passing through
//! the GUI manager.
//!
//! # Key Functions
//!
//! - [`show_settings_gui`] - Open the settings window
//! - [`show_gallery_gui`] - Open the clip gallery window
//! - [`run_clip_saved_overlay`] - Show a temporary "clip saved" overlay
//!
//! # Example
//!
//! ```ignore
//! use liteclip_replay::gui::{show_settings_gui, init_gui_manager};
//!
//! // Initialize the GUI manager (call once at startup)
//! init_gui_manager();
//!
//! // Show settings window
//! show_settings_gui(event_tx);
//! ```

pub mod manager;
pub use manager::{init_gui_manager, send_gui_message};

pub mod settings;
pub use settings::show_settings_gui;

pub mod gallery;
pub use gallery::show_gallery_gui;

pub mod clip_saved_overlay;
pub use clip_saved_overlay::run_clip_saved_overlay;
