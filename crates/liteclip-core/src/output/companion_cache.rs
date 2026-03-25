//! Sidecar path helpers for main clip files (thumbnail cache uses the same path hash).

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::Path;

/// Stable hash for a main video path (same algorithm as thumbnail cache).
pub fn hash_main_video_path(video_path: &Path) -> u64 {
    let mut hasher = DefaultHasher::new();
    video_path.hash(&mut hasher);
    hasher.finish()
}
