//! Game Detection
//!
//! This module provides functionality for detecting running games on the system.
//! Game detection is used to organize saved clips by game name.
//!
//! # How It Works
//!
//! The detector periodically scans running processes and matches them against
//! known game executables. When a game is detected, clips are saved in a
//! subdirectory named after the game.
//!
//! # Key Types
//!
//! - [`GameDetector`] - Main detector instance
//! - [`DetectedApp`] - Information about a detected application
//!
//! # Example
//!
//! ```no_run
//! use liteclip_replay::detection::GameDetector;
//!
//! let detector = GameDetector::new();
//! detector.start();
//!
//! // Later, check if a game is running
//! let app = detector.get_detected_app();
//! if app.is_game {
//!     println!("Detected game: {}", app.folder_name);
//! }
//! ```

pub mod game;

pub use game::{DetectedApp, GameDetector};
