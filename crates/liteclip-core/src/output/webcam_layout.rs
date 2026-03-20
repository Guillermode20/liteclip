//! Webcam picture-in-picture layout (sidecar JSON next to `.webcam-cache` companion MP4).

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use super::video_file::TimeRange;

/// Default PiP: bottom-right, ~18% width.
pub fn default_webcam_keyframes() -> Vec<WebcamKeyframe> {
    vec![WebcamKeyframe {
        t_secs: 0.0,
        x: 0.78,
        y: 0.72,
        w: 0.20,
        h: 0.22,
    }]
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebcamLayoutFile {
    pub keyframes: Vec<WebcamKeyframe>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebcamKeyframe {
    /// Time in seconds on the **main** video timeline.
    pub t_secs: f64,
    /// Normalized 0..1 relative to main frame (top-left of PiP).
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl WebcamLayoutFile {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let bytes = std::fs::read(path)?;
        let v: Self = serde_json::from_slice(&bytes)?;
        Ok(v)
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p)?;
        }
        let s = serde_json::to_string_pretty(self)?;
        std::fs::write(path, s)?;
        Ok(())
    }
}

/// Linear interpolation of normalized rect at time `t_secs` (main timeline).
pub fn interpolate_norm_rect(t_secs: f64, keyframes: &[WebcamKeyframe]) -> (f64, f64, f64, f64) {
    if keyframes.is_empty() {
        return (0.78, 0.72, 0.20, 0.22);
    }
    if keyframes.len() == 1 {
        let k = &keyframes[0];
        return (k.x, k.y, k.w, k.h);
    }
    let mut sorted: Vec<_> = keyframes.to_vec();
    sorted.sort_by(|a, b| {
        a.t_secs
            .partial_cmp(&b.t_secs)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    if t_secs <= sorted[0].t_secs {
        let k = &sorted[0];
        return (k.x, k.y, k.w, k.h);
    }
    if t_secs >= sorted[sorted.len() - 1].t_secs {
        let k = &sorted[sorted.len() - 1];
        return (k.x, k.y, k.w, k.h);
    }
    for w in sorted.windows(2) {
        let a = &w[0];
        let b = &w[1];
        if t_secs >= a.t_secs && t_secs <= b.t_secs {
            let span = (b.t_secs - a.t_secs).max(1e-6);
            let u = ((t_secs - a.t_secs) / span).clamp(0.0, 1.0);
            return (
                a.x + (b.x - a.x) * u,
                a.y + (b.y - a.y) * u,
                a.w + (b.w - a.w) * u,
                a.h + (b.h - a.h) * u,
            );
        }
    }
    let k = &sorted[sorted.len() - 1];
    (k.x, k.y, k.w, k.h)
}

/// Map a time on the **main** (source) timeline into **exported** timeline after applying `keep_ranges`.
pub fn source_time_to_output_time(source_t: f64, keep_ranges: &[TimeRange]) -> Option<f64> {
    let mut out = 0.0;
    for r in keep_ranges {
        if source_t < r.start_secs {
            return None;
        }
        if source_t < r.end_secs {
            return Some(out + (source_t - r.start_secs));
        }
        out += r.duration_secs();
    }
    None
}

/// Build keyframes with `t_secs` remapped to output timeline (for ffmpeg `t` in filter expressions).
pub fn keyframes_for_output_timeline(
    keyframes: &[WebcamKeyframe],
    keep_ranges: &[TimeRange],
) -> Vec<WebcamKeyframe> {
    keyframes
        .iter()
        .filter_map(|k| {
            source_time_to_output_time(k.t_secs, keep_ranges).map(|t| WebcamKeyframe {
                t_secs: t,
                x: k.x,
                y: k.y,
                w: k.w,
                h: k.h,
            })
        })
        .collect()
}

/// Sidecar path: same directory/hash as companion MP4; extension `.json`.
pub fn layout_path_for_companion_mp4(webcam_mp4: &Path) -> PathBuf {
    webcam_mp4.with_extension("json")
}
