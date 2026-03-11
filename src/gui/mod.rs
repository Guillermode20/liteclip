//! Graphical User Interface
//!
//! This module provides GUI components built with egui for settings
//! and clip gallery.
//!
//! # Components
//!
//! - **Settings GUI**: Configuration interface for video, audio, and hotkeys
//! - **Gallery GUI**: Browse and manage saved clips
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
//!
//! # Example
//!
//! ```no_run
//! use liteclip_replay::gui::{show_settings_gui, init_gui_manager};
//! use tokio::sync::mpsc::channel;
//!
//! // Initialize the GUI manager (call once at startup)
//! init_gui_manager();
//!
//! // Show settings window
//! let (tx, rx) = channel(1);
//! show_settings_gui(tx);
//! ```

pub mod manager;
pub use manager::{init_gui_manager, send_gui_message, show_toast, ToastKind};

pub mod settings;
pub use settings::show_settings_gui;

pub mod gallery;
pub use gallery::show_gallery_gui;
