//! Error and crash logging for LiteClip.
//!
//! Persists `ERROR` and `WARN` level tracing events to a rotating log file at
//! `%APPDATA%\liteclip\error.log`. Also installs a panic hook that appends crash
//! information to the same file.
//!
//! The log file is accessible from the Settings GUI "Logs" tab, where users can
//! view, copy, and clear logs.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use chrono::Local;
use tracing::{field::Visit, Level, Subscriber};
use tracing_subscriber::layer::Context as LayerContext;

const MAX_LOG_SIZE_BYTES: u64 = 5 * 1024 * 1024; // 5 MB

/// Handle used by the GUI to read, clear, and inspect the error log.
#[derive(Clone)]
pub struct FileLogGuard {
    log_path: PathBuf,
}

impl FileLogGuard {
    /// Create a guard for testing purposes with a temporary path.
    #[doc(hidden)]
    pub fn default_for_test() -> Self {
        Self {
            log_path: PathBuf::from("error.log"),
        }
    }

    /// Read the entire current error log. Returns empty string if the file does not exist.
    pub fn read_log(&self) -> String {
        fs::read_to_string(&self.log_path).unwrap_or_default()
    }

    /// Return `(modified_time, file_length)` via `fs::metadata`.
    /// Returns `None` if the file does not exist or metadata cannot be read.
    pub fn log_metadata(&self) -> Option<(std::time::SystemTime, u64)> {
        let meta = fs::metadata(&self.log_path).ok()?;
        Some((meta.modified().ok()?, meta.len()))
    }

    /// Truncate the error log to zero length.
    pub fn clear_log(&self) -> Result<()> {
        if self.log_path.exists() {
            fs::File::create(&self.log_path)
                .context("Failed to clear error log")?
                .set_len(0)
                .context("Failed to truncate error log")?;
        }
        Ok(())
    }

    /// Path to the log file directory (for "Open Folder" in GUI).
    pub fn log_dir(&self) -> &Path {
        self.log_path
            .parent()
            .expect("log_path always has a parent")
    }

    /// Path to the log file itself.
    pub fn log_path(&self) -> &Path {
        &self.log_path
    }

    /// Return a formatted copy-ready string with system info header.
    pub fn formatted_log(&self) -> String {
        use std::env::consts;

        let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S");
        let version = env!("CARGO_PKG_VERSION");

        let mut header = format!(
            "LiteClip v{version} Error Log\n\
             Generated: {timestamp}\n\
             OS: {os} {arch}\n\
             ---\n",
            os = consts::OS,
            arch = consts::ARCH,
        );

        header.push_str(&self.read_log());
        header
    }
}

/// A `tracing::Layer` that writes ERROR and WARN events to a file.
pub struct FileLogLayer {
    log_path: PathBuf,
}

impl FileLogLayer {
    fn new(log_path: PathBuf) -> Self {
        Self { log_path }
    }

    fn rotate_if_needed(&self) {
        let Ok(metadata) = fs::metadata(&self.log_path) else {
            return;
        };
        if metadata.len() < MAX_LOG_SIZE_BYTES {
            return;
        }

        let old_path = self.log_path.with_extension("log.old");
        let _ = fs::remove_file(&old_path);
        let _ = fs::rename(&self.log_path, &old_path);
    }

    fn append_line(&self, line: &str) {
        self.rotate_if_needed();

        let mut file = match fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log_path)
        {
            Ok(f) => f,
            Err(_) => return,
        };

        let _ = writeln!(file, "{line}");
    }
}

impl<S: Subscriber> tracing_subscriber::Layer<S> for FileLogLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: LayerContext<'_, S>) {
        let metadata = event.metadata();
        let level = *metadata.level();

        // Only capture WARN and ERROR
        if level != Level::ERROR && level != Level::WARN {
            return;
        }

        let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S");
        let level_str = if level == Level::ERROR {
            "ERROR"
        } else {
            "WARN "
        };

        // Collect the formatted message from the event's visitors
        let mut visitor = MessageCollector::default();
        event.record(&mut visitor);

        let file = metadata.file().unwrap_or("unknown");
        let line = metadata
            .line()
            .map(|l| l.to_string())
            .unwrap_or_else(|| "?".to_string());
        let target = metadata.target();

        self.append_line(&format!(
            "[{timestamp}] {level_str} {file}:{line} ({target}) - {msg}",
            msg = visitor.message,
        ));
    }
}

/// Collects the formatted message from a tracing event.
#[derive(Default)]
struct MessageCollector {
    message: String,
}

impl Visit for MessageCollector {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message = format!("{value:?}");
        } else {
            // Append extra fields
            if !self.message.is_empty() {
                self.message.push_str(", ");
            }
            self.message
                .push_str(&format!("{}={:?}", field.name(), value));
        }
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_string();
        } else {
            if !self.message.is_empty() {
                self.message.push_str(", ");
            }
            self.message
                .push_str(&format!("{}={:?}", field.name(), value));
        }
    }

    fn record_error(
        &mut self,
        field: &tracing::field::Field,
        value: &(dyn std::error::Error + 'static),
    ) {
        if field.name() == "message" || field.name() == "error" {
            self.message = format!("{value}");
        } else {
            self.message
                .push_str(&format!("{}={}", field.name(), value));
        }
    }
}

/// Initialize the error log system.
///
/// Must be called **before** `tracing_subscriber::init()` so the layer can be
/// registered. Also installs a panic hook that writes crash information to the
/// same log file.
///
/// # Arguments
/// * `config_dir` — Directory where `error.log` will be stored (e.g. `%APPDATA%\liteclip\`)
///
/// # Returns
/// A `FileLogGuard` for the GUI to read/clear the log, and a `FileLogLayer` to
/// add to the tracing subscriber.
pub fn init_error_log(config_dir: &Path) -> (FileLogGuard, FileLogLayer) {
    let log_path = config_dir.join("error.log");

    // Ensure the directory exists
    if let Err(e) = fs::create_dir_all(config_dir) {
        // Can't log this anywhere yet since tracing isn't initialized
        eprintln!(
            "Warning: failed to create log directory {:?}: {}",
            config_dir, e
        );
    }

    let guard = FileLogGuard {
        log_path: log_path.clone(),
    };
    let layer = FileLogLayer::new(log_path.clone());

    // Install panic hook that writes crash info to the log file
    let panic_log_path = log_path.clone();
    std::panic::set_hook(Box::new(move |panic_info| {
        let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S");

        let location = panic_info
            .location()
            .map(|loc| format!("{}:{}", loc.file(), loc.line()))
            .unwrap_or_else(|| "unknown location".to_string());

        let payload = panic_info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| panic_info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown panic payload".to_string());

        let backtrace = std::backtrace::Backtrace::capture();
        let backtrace_str = match backtrace.status() {
            std::backtrace::BacktraceStatus::Captured => format!("{backtrace}"),
            _ => String::from("(backtrace unavailable)"),
        };

        let crash_entry = format!(
            "\n======================== PANIC ========================\n\
             [{timestamp}] PANIC {location} - {payload}\n\
             Backtrace:\n{backtrace_str}\n\
             ======================================================\n",
        );

        if let Ok(mut file) = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&panic_log_path)
        {
            let _ = writeln!(file, "{crash_entry}");
        }

        // Also print to stderr for console visibility
        eprintln!("{crash_entry}");
    }));

    (guard, layer)
}

/// Create a shared (Arc) error log guard for passing through the GUI system.
///
/// Convenience wrapper around `init_error_log` that returns the guard wrapped
/// in `Arc` for sharing between the main loop and GUI.
pub fn init_error_log_shared(config_dir: &Path) -> (Arc<FileLogGuard>, FileLogLayer) {
    let (guard, layer) = init_error_log(config_dir);
    (Arc::new(guard), layer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn guard_reads_empty_when_no_file() {
        let temp = TempDir::new().unwrap();
        let guard = FileLogGuard {
            log_path: temp.path().join("error.log"),
        };
        assert_eq!(guard.read_log(), "");
    }

    #[test]
    fn guard_reads_existing_content() {
        let temp = TempDir::new().unwrap();
        let log_path = temp.path().join("error.log");
        fs::write(&log_path, "test log line\n").unwrap();

        let guard = FileLogGuard { log_path: log_path };
        assert_eq!(guard.read_log(), "test log line\n");
    }

    #[test]
    fn guard_clears_log() {
        let temp = TempDir::new().unwrap();
        let log_path = temp.path().join("error.log");
        fs::write(&log_path, "some content").unwrap();

        let guard = FileLogGuard { log_path: log_path };
        guard.clear_log().unwrap();
        assert_eq!(guard.read_log(), "");
    }

    #[test]
    fn guard_clear_nonexistent_is_ok() {
        let temp = TempDir::new().unwrap();
        let guard = FileLogGuard {
            log_path: temp.path().join("nonexistent.log"),
        };
        assert!(guard.clear_log().is_ok());
    }

    #[test]
    fn guard_log_dir_returns_parent() {
        let temp = TempDir::new().unwrap();
        let log_path = temp.path().join("error.log");
        let guard = FileLogGuard { log_path: log_path };
        assert_eq!(guard.log_dir(), temp.path());
    }

    #[test]
    fn guard_log_path_returns_path() {
        let temp = TempDir::new().unwrap();
        let log_path = temp.path().join("error.log");
        let guard = FileLogGuard {
            log_path: log_path.clone(),
        };
        assert_eq!(guard.log_path(), log_path);
    }

    #[test]
    fn formatted_log_includes_header() {
        let temp = TempDir::new().unwrap();
        let log_path = temp.path().join("error.log");
        fs::write(&log_path, "[test] ERROR - something broke\n").unwrap();

        let guard = FileLogGuard { log_path: log_path };
        let formatted = guard.formatted_log();

        assert!(formatted.contains("LiteClip v"));
        assert!(formatted.contains("Error Log"));
        assert!(formatted.contains("Generated:"));
        assert!(formatted.contains("OS:"));
        assert!(formatted.contains("---"));
        assert!(formatted.contains("[test] ERROR - something broke"));
    }

    #[test]
    fn rotate_removes_old_and_renames() {
        let temp = TempDir::new().unwrap();
        let log_path = temp.path().join("error.log");
        let old_path = temp.path().join("error.log.old");

        // Write a file larger than the limit
        let big_content = "x".repeat((MAX_LOG_SIZE_BYTES + 1) as usize);
        fs::write(&log_path, &big_content).unwrap();

        let layer = FileLogLayer::new(log_path.clone());
        layer.rotate_if_needed();

        assert!(old_path.exists(), "old file should exist after rotation");
        assert!(
            fs::metadata(&old_path).unwrap().len() > MAX_LOG_SIZE_BYTES,
            "old file should contain the previous large content"
        );
    }

    #[test]
    fn message_collector_default_is_empty() {
        let collector = MessageCollector::default();
        assert!(collector.message.is_empty());
    }
}
