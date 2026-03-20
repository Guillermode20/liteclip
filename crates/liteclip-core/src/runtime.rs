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
/// On failure, `Err` contains the `path` argument that was **not** stored because an
/// override was already installed ([`std::sync::OnceLock::set`]).
pub fn set_ffmpeg_path(path: PathBuf) -> Result<(), PathBuf> {
    FFMPEG_PATH_OVERRIDE.set(path)
}

fn push_ffmpeg_dev_candidates(start: &Path, out: &mut Vec<PathBuf>) {
    for dir in start.ancestors().take(10) {
        out.push(
            dir
                .join("ffmpeg_dev")
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
