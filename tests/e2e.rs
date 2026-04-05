//! End-to-end integration tests for LiteClip.
//!
//! This is the main entry point for all e2e tests. Individual test modules
//! are organized by category in the `e2e/` directory.
//!
//! ## Running E2E Tests
//!
//! ```bash
//! # All e2e tests
//! cargo test --test e2e --features ffmpeg
//!
//! # Specific category
//! cargo test --test e2e app_lifecycle
//! cargo test --test e2e workflow
//!
//! # With output
//! cargo test --test e2e --features ffmpeg -- --nocapture
//! ```
//!
//! ## Test Organization
//!
//! - `app_lifecycle.rs` - App initialization, start/stop, shutdown
//! - `workflows.rs` - User scenarios and complete workflows
//! - `gui_interactions.rs` - GUI component tests

#![cfg(feature = "ffmpeg")]

mod common;

// Re-export e2e test modules
mod e2e {
    pub mod app_lifecycle;
    pub mod gui_interactions;
    pub mod workflows;
}

// Re-export common utilities for test visibility
pub use common::*;
