//! Sidecar paths for main clip files (webcam companion video, layout JSON).
//!
//! Uses the same path-hash scheme as gallery thumbnails ([`super::sdk_ffmpeg_output::generate_thumbnail`]).

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

/// Stable hash for a main video path (same algorithm as thumbnail cache).
pub fn hash_main_video_path(video_path: &Path) -> u64 {
    let mut hasher = DefaultHasher::new();
    video_path.hash(&mut hasher);
    hasher.finish()
}

/// `{save_directory}/.webcam-cache`
pub fn webcam_cache_dir(save_directory: &Path) -> PathBuf {
    save_directory.join(".webcam-cache")
}

/// `{save_directory}/.webcam-cache/{hash:016x}.mp4`
pub fn webcam_video_path(save_directory: &Path, main_video_path: &Path) -> PathBuf {
    let h = hash_main_video_path(main_video_path);
    webcam_cache_dir(save_directory).join(format!("{h:016x}.mp4"))
}

/// `{save_directory}/.webcam-cache/{hash:016x}.json` — PiP layout keyframes.
pub fn webcam_layout_path(save_directory: &Path, main_video_path: &Path) -> PathBuf {
    let h = hash_main_video_path(main_video_path);
    webcam_cache_dir(save_directory).join(format!("{h:016x}.json"))
}
