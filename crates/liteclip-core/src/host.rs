//! Optional embedder callbacks (toasts, UI) without depending on `tracing` alone.

use std::path::Path;

/// Host integration hooks for clip save and pipeline failure.
///
/// All methods have default no-op implementations.
///
/// **Recommended wiring:** Install one `Arc<dyn CoreHost>` with [`crate::ReplayEngine::set_core_host`]
/// for [`CoreHost::on_pipeline_fatal`], and pass the same `Arc` to [`crate::ReplayEngine::save_clip`]
/// for [`CoreHost::on_clip_saved`]. You can also use [`crate::app::AppState::set_core_host`] on
/// [`crate::ReplayEngine::state_mut`] if you prefer.
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    struct TestHost;
    impl CoreHost for TestHost {}

    #[test]
    fn default_on_clip_saved_is_noop() {
        let host = TestHost;
        host.on_clip_saved(Path::new("/test/path.mp4"));
    }

    #[test]
    fn default_on_pipeline_fatal_is_noop() {
        let host = TestHost;
        host.on_pipeline_fatal("test reason");
    }

    #[test]
    fn mock_host_records_clip_saved() {
        struct RecordingHost {
            count: AtomicUsize,
        }
        impl CoreHost for RecordingHost {
            fn on_clip_saved(&self, _path: &Path) {
                self.count.fetch_add(1, Ordering::SeqCst);
            }
        }

        let host = Arc::new(RecordingHost {
            count: AtomicUsize::new(0),
        });
        host.on_clip_saved(Path::new("/clip.mp4"));
        host.on_clip_saved(Path::new("/clip2.mp4"));
        assert_eq!(host.count.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn mock_host_records_pipeline_fatal() {
        struct FatalHost {
            last_reason: std::sync::Mutex<String>,
        }
        impl CoreHost for FatalHost {
            fn on_pipeline_fatal(&self, reason: &str) {
                *self.last_reason.lock().unwrap() = reason.to_string();
            }
        }

        let host = FatalHost {
            last_reason: std::sync::Mutex::new(String::new()),
        };
        host.on_pipeline_fatal("encoder crashed");
        assert_eq!(*host.last_reason.lock().unwrap(), "encoder crashed");
    }
}
