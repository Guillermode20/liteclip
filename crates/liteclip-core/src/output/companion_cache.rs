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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_is_deterministic() {
        let path = Path::new("C:/Videos/clips/test.mp4");
        let hash1 = hash_main_video_path(path);
        let hash2 = hash_main_video_path(path);
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn different_paths_produce_different_hashes() {
        let path1 = Path::new("C:/Videos/clips/video1.mp4");
        let path2 = Path::new("C:/Videos/clips/video2.mp4");
        assert_ne!(hash_main_video_path(path1), hash_main_video_path(path2));
    }

    #[test]
    fn case_sensitive_paths() {
        let path1 = Path::new("C:/Videos/Clip.mp4");
        let path2 = Path::new("C:/Videos/clip.mp4");
        assert_ne!(hash_main_video_path(path1), hash_main_video_path(path2));
    }
}
