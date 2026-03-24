use eframe::egui;
use image::RgbaImage;
use std::path::{Path, PathBuf};

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

use super::{
    EditorState, SnippetSegment, ThumbnailStrip, TimeRange, VideoEntry, MIN_RANGE_SECS,
    THUMBNAIL_STRIP_COUNT, THUMBNAIL_STRIP_WIDTH,
};
use crate::output::{estimate_export_bitrates, VideoFileMetadata};

#[cfg(all(target_os = "windows", not(feature = "ffmpeg")))]
const CREATE_NO_WINDOW: u32 = 0x08000000;

pub(super) fn collect_video_paths_impl(
    dir: &Path,
    cache_dir: &Path,
    webcam_cache_dir: &Path,
    output: &mut Vec<PathBuf>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path == cache_dir || path == webcam_cache_dir {
            continue;
        }
        if path.is_dir() {
            collect_video_paths_impl(&path, cache_dir, webcam_cache_dir, output);
        } else if path
            .extension()
            .map(|ext| ext.eq_ignore_ascii_case("mp4"))
            .unwrap_or(false)
        {
            output.push(path);
        }
    }
}

pub(super) fn open_path_impl(path: &Path) -> anyhow::Result<()> {
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/C", "start", "", &path.to_string_lossy()])
            .creation_flags(0x08000000)
            .spawn()
            .map_err(anyhow::Error::from)?;
        Ok(())
    }

    #[cfg(not(target_os = "windows"))]
    {
        std::process::Command::new("xdg-open")
            .arg(path)
            .spawn()
            .map_err(anyhow::Error::from)?;
        Ok(())
    }
}

pub(super) fn build_clipped_output_path_impl(video: &VideoEntry) -> PathBuf {
    let stem = video
        .path
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or_else(|| "clip".to_string());

    if video.is_external {
        let output_dir = video
            .path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| video.save_root.clone());
        let _ = std::fs::create_dir_all(&output_dir);

        for attempt in 0..1000 {
            let suffix = if attempt == 0 {
                "_clipped".to_string()
            } else {
                format!("_clipped_{attempt}")
            };
            let candidate = output_dir.join(format!("{stem}{suffix}.mp4"));
            if !candidate.exists() {
                return candidate;
            }
        }

        return output_dir.join(format!(
            "{}_clipped_{}.mp4",
            stem,
            chrono::Local::now().format("%Y%m%d_%H%M%S")
        ));
    }

    let game_folder = format!("Clipped-{}", video.game.replace(['\\', '/'], "-"));
    let output_dir = video.save_root.join(game_folder);
    let _ = std::fs::create_dir_all(&output_dir);

    for attempt in 0..1000 {
        let suffix = if attempt == 0 {
            "_clipped".to_string()
        } else {
            format!("_clipped_{attempt}")
        };
        let candidate = output_dir.join(format!("{stem}{suffix}.mp4"));
        if !candidate.exists() {
            return candidate;
        }
    }

    output_dir.join(format!(
        "{}_clipped_{}.mp4",
        stem,
        chrono::Local::now().format("%Y%m%d_%H%M%S")
    ))
}

pub(super) fn load_rgba_image_from_path_impl(path: &Path) -> anyhow::Result<RgbaImage> {
    Ok(image::open(path)?.into_rgba8())
}

pub(super) fn color_image_from_rgba_impl(image: &RgbaImage) -> egui::ColorImage {
    egui::ColorImage::from_rgba_unmultiplied(
        [image.width() as usize, image.height() as usize],
        image.as_raw(),
    )
}

pub(super) fn format_size_mb_impl(size_mb: f64) -> String {
    if size_mb >= 100.0 {
        format!("{size_mb:.0} MB")
    } else {
        format!("{size_mb:.1} MB")
    }
}

pub(super) fn format_compact_duration_impl(seconds: f64) -> String {
    let total_seconds = seconds.max(0.0).round() as u64;
    let hours = total_seconds / 3600;
    let minutes = (total_seconds / 60) % 60;
    let secs = total_seconds % 60;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{secs:02}")
    } else {
        format!("{minutes}:{secs:02}")
    }
}

pub(super) fn format_timestamp_precise_impl(seconds: f64) -> String {
    let total_millis = (seconds.max(0.0) * 1000.0).round() as u64;
    let hours = total_millis / 3_600_000;
    let minutes = (total_millis / 60_000) % 60;
    let secs = (total_millis / 1000) % 60;
    let millis = total_millis % 1000;
    format!("{hours:02}:{minutes:02}:{secs:02}.{millis:03}")
}

fn normalize_cut_points_impl(cut_points: &mut Vec<f64>, duration_secs: f64) {
    cut_points.retain(|point| *point > MIN_RANGE_SECS && *point < duration_secs - MIN_RANGE_SECS);
    cut_points.sort_by(|a, b| a.total_cmp(b));
    cut_points.dedup_by(|a, b| (*a - *b).abs() < MIN_RANGE_SECS);
}

pub(super) fn clear_cut_points_impl(editor: &mut EditorState) {
    editor.cut_points.clear();
    editor.snippet_enabled.clear();
    editor.snippet_enabled.push(true);
    editor.selected_cut_point = None;
}

pub(super) fn add_cut_point_impl(editor: &mut EditorState, time_secs: f64) -> bool {
    let duration = editor.duration_secs();
    let cut_time = time_secs.clamp(0.0, duration);
    if cut_time <= MIN_RANGE_SECS || cut_time >= duration - MIN_RANGE_SECS {
        editor.error_message = Some("Cuts must stay inside the clip boundaries".to_string());
        return false;
    }

    let insert_index = match editor
        .cut_points
        .binary_search_by(|probe| probe.total_cmp(&cut_time))
    {
        Ok(_) => {
            editor.error_message = Some("A cut already exists near that point".to_string());
            return false;
        }
        Err(index) => index,
    };

    let previous_boundary = if insert_index == 0 {
        0.0
    } else {
        editor.cut_points[insert_index - 1]
    };
    let next_boundary = editor
        .cut_points
        .get(insert_index)
        .copied()
        .unwrap_or(duration);
    if cut_time - previous_boundary < MIN_RANGE_SECS || next_boundary - cut_time < MIN_RANGE_SECS {
        editor.error_message = Some("Cuts must leave each snippet with some duration".to_string());
        return false;
    }

    let inherited = editor
        .snippet_enabled
        .get(insert_index)
        .copied()
        .unwrap_or(true);
    editor.cut_points.insert(insert_index, cut_time);
    editor.snippet_enabled.insert(insert_index + 1, inherited);
    normalize_cut_points_impl(&mut editor.cut_points, duration);
    editor.selected_cut_point = Some(insert_index);
    editor.error_message = None;
    true
}

pub(super) fn remove_cut_point_impl(editor: &mut EditorState, index: usize) {
    if index >= editor.cut_points.len() {
        return;
    }

    editor.cut_points.remove(index);
    let right_enabled = if index + 1 < editor.snippet_enabled.len() {
        editor.snippet_enabled.remove(index + 1)
    } else {
        true
    };
    if let Some(left_enabled) = editor.snippet_enabled.get_mut(index) {
        *left_enabled = *left_enabled || right_enabled;
    }
    editor.selected_cut_point = index
        .checked_sub(1)
        .or(Some(index).filter(|i| *i < editor.cut_points.len()));
    editor.error_message = None;
}

pub(super) fn snippet_segments_impl(
    duration_secs: f64,
    cut_points: &[f64],
    snippet_enabled: &[bool],
) -> Vec<SnippetSegment> {
    let mut segments = Vec::with_capacity(cut_points.len() + 1);
    let mut start_secs = 0.0;

    for (index, end_secs) in cut_points
        .iter()
        .copied()
        .chain(std::iter::once(duration_secs))
        .enumerate()
    {
        let enabled = snippet_enabled.get(index).copied().unwrap_or(true);
        segments.push(SnippetSegment {
            start_secs,
            end_secs,
            enabled,
        });
        start_secs = end_secs;
    }

    segments
}

pub(super) fn enabled_time_ranges_impl(
    duration_secs: f64,
    cut_points: &[f64],
    snippet_enabled: &[bool],
) -> Vec<TimeRange> {
    snippet_segments_impl(duration_secs, cut_points, snippet_enabled)
        .into_iter()
        .filter(|segment| segment.enabled && segment.duration_secs() >= MIN_RANGE_SECS)
        .map(|segment| TimeRange {
            start_secs: segment.start_secs,
            end_secs: segment.end_secs,
        })
        .collect()
}

pub(super) fn clamp_to_enabled_playback_time_impl(
    current_time_secs: f64,
    duration_secs: f64,
    cut_points: &[f64],
    snippet_enabled: &[bool],
) -> f64 {
    let segments = snippet_segments_impl(duration_secs, cut_points, snippet_enabled);

    let current_snippet_index = segments
        .iter()
        .position(|s| current_time_secs >= s.start_secs && current_time_secs < s.end_secs)
        .unwrap_or(0);

    if segments
        .get(current_snippet_index)
        .map(|s| s.enabled)
        .unwrap_or(false)
    {
        return current_time_secs;
    }

    for segment in segments.iter().skip(current_snippet_index + 1) {
        if segment.enabled {
            return segment.start_secs;
        }
    }

    for segment in segments.iter().take(current_snippet_index).rev() {
        if segment.enabled {
            return segment.start_secs;
        }
    }

    current_time_secs
}

pub(super) fn estimate_export_bitrates_from_editor_impl(
    target_size_mb: u32,
    kept_duration_secs: f64,
    has_audio: bool,
    requested_audio_bitrate_kbps: u32,
    num_segments: usize,
    use_hardware_acceleration: bool,
) -> (u32, u32) {
    let estimate = estimate_export_bitrates(
        target_size_mb,
        kept_duration_secs,
        has_audio,
        requested_audio_bitrate_kbps,
        num_segments,
        use_hardware_acceleration,
    );

    (estimate.video_kbps, estimate.total_kbps)
}

pub(super) fn quality_estimate_impl(
    metadata: &VideoFileMetadata,
    video_kbps: u32,
) -> (&'static str, usize) {
    let pixel_factor = (metadata.width as f64 * metadata.height as f64) / (1920.0 * 1080.0);

    let fps_factor = (metadata.fps / 30.0).clamp(0.5, 3.0);

    let combined_factor = pixel_factor * fps_factor;

    let medium_threshold = 2000.0 * combined_factor;
    let high_threshold = 5000.0 * combined_factor;
    let bitrate = video_kbps as f64;

    if bitrate >= high_threshold {
        ("High", 5)
    } else if bitrate >= medium_threshold {
        ("Medium", 3)
    } else {
        ("Low", 2)
    }
}

pub(super) fn time_to_x_impl(rect: egui::Rect, time_secs: f64, duration_secs: f64) -> f32 {
    let ratio = if duration_secs <= 0.0 {
        0.0
    } else {
        (time_secs / duration_secs).clamp(0.0, 1.0) as f32
    };
    egui::lerp(rect.left()..=rect.right(), ratio)
}

pub(super) fn x_to_time_impl(rect: egui::Rect, x: f32, duration_secs: f64) -> f64 {
    if rect.width() <= 0.0 || duration_secs <= 0.0 {
        return 0.0;
    }
    let ratio = ((x - rect.left()) / rect.width()).clamp(0.0, 1.0);
    duration_secs * f64::from(ratio)
}

pub(super) fn generate_thumbnail_strip_frames_impl(
    video_path: &Path,
    duration_secs: f64,
    _has_audio: bool,
) -> anyhow::Result<ThumbnailStrip> {
    generate_thumbnail_strip_frames_sdk(video_path, duration_secs)
}

#[cfg(feature = "ffmpeg")]
fn generate_thumbnail_strip_frames_sdk(
    video_path: &Path,
    duration_secs: f64,
) -> anyhow::Result<ThumbnailStrip> {
    use crate::output::sdk_ffmpeg_output::extract_preview_frame;
    use tracing::{debug, warn};

    let mut thumbnails = Vec::with_capacity(THUMBNAIL_STRIP_COUNT);

    if duration_secs <= 0.0 {
        return Ok(ThumbnailStrip::new(thumbnails, duration_secs));
    }

    let frame_times: Vec<f64> = (1..=THUMBNAIL_STRIP_COUNT)
        .map(|i| duration_secs * (i as f64) / (THUMBNAIL_STRIP_COUNT + 1) as f64)
        .collect();

    for (i, time_secs) in frame_times.iter().enumerate() {
        debug!("Extracting thumbnail strip frame {}/{} at {:.2}s", i + 1, THUMBNAIL_STRIP_COUNT, time_secs);
        match extract_preview_frame(video_path, *time_secs, THUMBNAIL_STRIP_WIDTH) {
            Ok(frame) => {
                thumbnails.push((*time_secs, frame));
            }
            Err(e) => {
                warn!("Failed to extract frame at {:.2}s: {}", time_secs, e);
                if let Some(last) = thumbnails.last() {
                    thumbnails.push((frame_times[i], last.1.clone()));
                }
            }
        }
    }

    while thumbnails.len() < THUMBNAIL_STRIP_COUNT {
        if let Some(last) = thumbnails.last() {
            let idx = thumbnails.len();
            let time = duration_secs * (idx as f64) / (THUMBNAIL_STRIP_COUNT as f64);
            thumbnails.push((time, last.1.clone()));
        } else {
            break;
        }
    }

    Ok(ThumbnailStrip::new(thumbnails, duration_secs))
}
