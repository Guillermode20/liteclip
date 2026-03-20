//! Optional embedder callbacks (toasts, UI) without depending on `tracing` alone.

use std::path::Path;

/// Host integration hooks for clip save and pipeline failure.
///
/// All methods have default no-op implementations.
///
/// **Pipeline fatals:** If you handle `Ok(Some(reason))` from
/// [`crate::app::AppState::enforce_pipeline_health`] in your UI, you typically either
/// implement `on_pipeline_fatal` *or* branch on the return value — not both for the same
/// notification.
pub trait CoreHost: Send + Sync {
    /// Called after a clip file was written successfully.
    fn on_clip_saved(&self, _path: &Path) {}

    /// Called when the recording pipeline hits a fatal error and has stopped.
    fn on_pipeline_fatal(&self, _reason: &str) {}
}
