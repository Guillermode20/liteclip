//! Runtime overrides (FFmpeg binary path, environment).
//!
//! # FFmpeg resolution order
//!
//! 1. Environment variable [`FFMPEG_ENV`] if set and the path exists.
//! 2. [`set_ffmpeg_path`] if set and the path exists.
//! 3. `ffmpeg.exe` next to the current process executable (if present).
//! 4. In **dev** builds (`debug_assertions`) or with feature `dev-ffmpeg-paths`: walk parent
//!    directories of the executable and of `CARGO_MANIFEST_DIR`, and use the first existing
//!    `ffmpeg_dev\sdk\bin\ffmpeg.exe` found (supports monorepo and a standalone crate repo).
//! 5. Fall back to `ffmpeg` on `PATH`.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

static FFMPEG_PATH_OVERRIDE: OnceLock<PathBuf> = OnceLock::new();

/// Environment variable: absolute path to `ffmpeg` or `ffmpeg.exe`.
pub const FFMPEG_ENV: &str = "LITECLIP_CORE_FFMPEG";

/// Set the FFmpeg executable path for this process (first successful call wins).
///
/// Returns `Ok(())` when `path` was stored, or `Err(path)` when an override was already set
/// ([`std::sync::OnceLock::set`]). This is **not** a path-validation error; callers should check
/// `path.exists()` themselves if needed.
pub fn set_ffmpeg_path(path: PathBuf) -> Result<(), PathBuf> {
    FFMPEG_PATH_OVERRIDE.set(path)
}

fn push_ffmpeg_dev_candidates(start: &Path, out: &mut Vec<PathBuf>) {
    for dir in start.ancestors().take(10) {
        out.push(
            dir.join("ffmpeg_dev")
                .join("sdk")
                .join("bin")
                .join("ffmpeg.exe"),
        );
    }
}

pub(crate) fn resolve_ffmpeg_executable() -> PathBuf {
    if let Ok(raw) = std::env::var(FFMPEG_ENV) {
        let p = PathBuf::from(raw.trim());
        if p.exists() {
            return p;
        }
    }

    if let Some(p) = FFMPEG_PATH_OVERRIDE.get() {
        if p.exists() {
            return p.clone();
        }
    }

    let mut candidates: Vec<PathBuf> = Vec::new();

    if let Ok(current_exe) = std::env::current_exe() {
        if let Some(exe_dir) = current_exe.parent() {
            candidates.push(exe_dir.join("ffmpeg.exe"));
            if cfg!(any(debug_assertions, feature = "dev-ffmpeg-paths")) {
                push_ffmpeg_dev_candidates(exe_dir, &mut candidates);
            }
        }
    }

    #[cfg(any(debug_assertions, feature = "dev-ffmpeg-paths"))]
    {
        push_ffmpeg_dev_candidates(Path::new(env!("CARGO_MANIFEST_DIR")), &mut candidates);
    }

    candidates
        .into_iter()
        .find(|path| path.exists())
        .unwrap_or_else(|| PathBuf::from("ffmpeg"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Serialize tests that touch `LITECLIP_CORE_FFMPEG`.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn resolve_prefers_ffmpeg_env_when_file_exists() {
        let _guard = ENV_LOCK.lock().expect("env test lock");

        let dir = std::env::temp_dir().join(format!("lc_ffmpeg_rt_{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let fake = dir.join("ffmpeg.exe");
        std::fs::write(&fake, b"x").expect("write fake ffmpeg");

        std::env::set_var(FFMPEG_ENV, fake.as_os_str());
        let got = resolve_ffmpeg_executable();
        std::env::remove_var(FFMPEG_ENV);

        assert_eq!(got, fake);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
