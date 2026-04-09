use crate::config::EncoderType;
use crate::encode::{resolve_effective_encoder_config, EncoderConfig};
use crate::quality_contracts::{validate_export_validity, ExportValidationInput};
use anyhow::{bail, Context, Result};
use image::RgbaImage;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::Sender,
    Arc,
};
use std::thread;
use std::time::Instant;
use tracing::{error, info, warn};

#[derive(Debug, Clone)]
pub struct VideoFileMetadata {
    pub duration_secs: f64,
    pub width: u32,
    pub height: u32,
    pub has_audio: bool,
    pub fps: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimeRange {
    pub start_secs: f64,
    pub end_secs: f64,
}

impl TimeRange {
    pub fn duration_secs(self) -> f64 {
        (self.end_secs - self.start_secs).max(0.0)
    }
}

/// Rectangular crop region in source video pixel coordinates.
///
/// All values are in the original video's pixel space (before any scaling).
/// Width and height must be even numbers (divisible by 2) for H.265 encoding.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CropRect {
    /// Horizontal offset from left edge in pixels.
    pub x: u32,
    /// Vertical offset from top edge in pixels.
    pub y: u32,
    /// Crop width in pixels.
    pub width: u32,
    /// Crop height in pixels.
    pub height: u32,
}

impl CropRect {
    /// Minimum allowed crop dimension (pixels).
    pub const MIN_SIZE: u32 = 64;

    /// Clamp the crop rect to fit within the given video dimensions,
    /// rounding down to even numbers.
    pub fn clamped_to(self, video_width: u32, video_height: u32) -> Self {
        let max_w = video_width & !1;
        let max_h = video_height & !1;
        let x = self.x.min(max_w.saturating_sub(Self::MIN_SIZE)) & !1;
        let y = self.y.min(max_h.saturating_sub(Self::MIN_SIZE)) & !1;
        let width = self.width.min(max_w.saturating_sub(x)) & !1;
        let height = self.height.min(max_h.saturating_sub(y)) & !1;
        Self {
            x,
            y,
            width: width.max(Self::MIN_SIZE & !1),
            height: height.max(Self::MIN_SIZE & !1),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipExportPhase {
    Preparing,
    Calibration,
    FirstPass,
    SecondPass,
}

#[derive(Debug, Clone)]
pub enum ClipExportUpdate {
    Progress {
        phase: ClipExportPhase,
        fraction: f32,
        message: String,
    },
    Finished(PathBuf),
    Failed(String),
    Cancelled,
}

#[derive(Debug, Clone)]
pub struct ClipExportRequest {
    pub input_path: PathBuf,
    pub output_path: PathBuf,
    pub keep_ranges: Vec<TimeRange>,
    pub target_size_mb: u32,
    pub audio_bitrate_kbps: u32,
    pub use_hardware_acceleration: bool,
    pub preferred_encoder: EncoderType,
    pub metadata: VideoFileMetadata,
    /// If true, use stream copy (no re-encoding) for fastest export preserving original quality.
    /// Used when user hasn't manually adjusted the target size.
    pub stream_copy: bool,
    /// Output resolution. None means original resolution.
    pub output_width: Option<u32>,
    pub output_height: Option<u32>,
    /// Output frame rate. None means use original.
    pub output_fps: Option<f64>,
    /// Spatial crop region. None means no cropping (full frame).
    /// When set, stream_copy is forced off since cropping requires re-encoding.
    pub crop: Option<CropRect>,
    /// Enable adaptive post-processing filters (deblocking, sharpening, contrast) during export.
    /// Filter strengths are computed automatically from bitrate/resolution.
    pub post_process_filters: bool,
}

impl ClipExportRequest {
    pub fn output_duration_secs(&self) -> f64 {
        self.keep_ranges
            .iter()
            .map(|range| range.duration_secs())
            .sum()
    }
}

pub enum ExportOutcome {
    Finished(PathBuf),
    Cancelled,
}

#[derive(Debug, Clone)]
pub struct ExportAttemptResult {
    pub output_path: PathBuf,
    pub video_bitrate_kbps: u32,
    pub size_bytes: u64,
}

#[derive(Debug, Clone)]
pub struct ExportBitrateEstimate {
    pub video_kbps: u32,
    pub audio_kbps: u32,
    pub total_kbps: u32,
}

#[derive(Debug, Clone, Copy)]
struct CalibrationPoint {
    video_bitrate_kbps: u32,
    total_output_bytes: u64,
    sample_duration_secs: f64,
    sample_segments: usize,
}

#[derive(Debug, Clone, Copy)]
struct SizeBudget {
    target_size_bytes: u64,
    output_duration_secs: f64,
    audio_bitrate_kbps: u32,
    estimated_non_video_bytes: u64,
    target_video_bytes: u64,
    initial_video_bitrate_kbps: u32,
}

impl SizeBudget {
    fn from_request(request: &ClipExportRequest, encoder: ExportVideoEncoder) -> Self {
        let output_duration_secs = request.output_duration_secs().max(0.1);
        let output_complexity_ratio = export_output_complexity_ratio(request);
        let bitrate_estimate = estimate_export_bitrates_for_encoder(
            request.target_size_mb,
            output_duration_secs,
            request.metadata.has_audio,
            request.audio_bitrate_kbps,
            request.keep_ranges.len(),
            encoder,
            output_complexity_ratio,
        );
        let target_size_bytes = target_size_bytes(request.target_size_mb);
        let estimated_non_video_bytes = estimate_non_video_bytes(
            output_duration_secs,
            bitrate_estimate.audio_kbps,
            request.keep_ranges.len(),
        );
        let target_video_bytes = target_size_bytes
            .saturating_sub(estimated_non_video_bytes)
            .max(1);

        Self {
            target_size_bytes,
            output_duration_secs,
            audio_bitrate_kbps: bitrate_estimate.audio_kbps,
            estimated_non_video_bytes,
            target_video_bytes,
            initial_video_bitrate_kbps: bitrate_estimate
                .video_kbps
                .max(encoder.min_video_bitrate_kbps()),
        }
    }
}

const MIN_VIDEO_BITRATE_KBPS: u32 = 300;
const MAX_VIDEO_BITRATE_KBPS: u32 = 400_000;
const MIN_REASONABLE_FPS: f64 = 1.0;
const MAX_REASONABLE_FPS: f64 = 240.0;
const FALLBACK_EXPORT_FPS: f64 = 60.0;
const TARGET_FILL_MIN_RATIO: f64 = 0.90;
const TARGET_FILL_MAX_RATIO: f64 = 1.00;
const INITIAL_TARGET_FILL_RATIO: f64 = 0.96;
const CALIBRATION_TARGET_FILL_RATIO: f64 = 0.97;
const RETRY_TARGET_FILL_RATIO: f64 = 0.985;
const CALIBRATION_MIN_CLIP_DURATION_SECS: f64 = 6.0;
const CALIBRATION_SAMPLE_RATIO: f64 = 0.25;
const CALIBRATION_MIN_SAMPLE_SECS: f64 = 4.0;
const CALIBRATION_MAX_SAMPLE_SECS: f64 = 18.0;
const CALIBRATION_LOW_FACTOR: f64 = 0.55;
const CALIBRATION_HIGH_FACTOR: f64 = 1.45;
const CALIBRATION_POWER_EXPONENT_MIN: f64 = 0.35;
const CALIBRATION_POWER_EXPONENT_MAX: f64 = 2.0;
const MAX_EXPORT_ATTEMPTS: usize = 8;
const BITRATE_NUDGE_KBPS: u32 = 24;
const COMPLEXITY_RATIO_MIN: f64 = 0.10;
const COMPLEXITY_RATIO_MAX: f64 = 2.0;

fn source_dimensions_for_export(request: &ClipExportRequest) -> (u32, u32) {
    let (width, height) = if let Some(crop) = request.crop {
        (crop.width, crop.height)
    } else {
        (request.metadata.width, request.metadata.height)
    };

    (width.max(2), height.max(2))
}

fn resolved_output_dimensions(request: &ClipExportRequest) -> (u32, u32) {
    let (source_width, source_height) = source_dimensions_for_export(request);
    (
        request.output_width.unwrap_or(source_width).max(2),
        request.output_height.unwrap_or(source_height).max(2),
    )
}

fn resolved_output_fps_for_request(request: &ClipExportRequest) -> f64 {
    let source_fps = normalize_output_fps(request.metadata.fps, FALLBACK_EXPORT_FPS);
    normalize_output_fps(request.output_fps.unwrap_or(source_fps), source_fps)
}

fn export_output_complexity_ratio(request: &ClipExportRequest) -> f64 {
    let (source_width, source_height) = source_dimensions_for_export(request);
    let (output_width, output_height) = resolved_output_dimensions(request);
    let source_fps = normalize_output_fps(request.metadata.fps, FALLBACK_EXPORT_FPS);
    let output_fps = resolved_output_fps_for_request(request);

    let source_pixel_rate =
        source_width as f64 * source_height as f64 * source_fps.max(MIN_REASONABLE_FPS);
    let output_pixel_rate =
        output_width as f64 * output_height as f64 * output_fps.max(MIN_REASONABLE_FPS);

    if source_pixel_rate <= 0.0 {
        1.0
    } else {
        (output_pixel_rate / source_pixel_rate).clamp(COMPLEXITY_RATIO_MIN, COMPLEXITY_RATIO_MAX)
    }
}

pub(crate) fn normalize_output_fps(fps: f64, fallback: f64) -> f64 {
    let fallback =
        if fallback.is_finite() && (MIN_REASONABLE_FPS..=MAX_REASONABLE_FPS).contains(&fallback) {
            fallback
        } else {
            FALLBACK_EXPORT_FPS
        };

    if fps.is_finite() && (MIN_REASONABLE_FPS..=MAX_REASONABLE_FPS).contains(&fps) {
        fps
    } else {
        fallback
    }
}

pub fn probe_video_file(video_path: &Path) -> Result<VideoFileMetadata> {
    #[cfg(feature = "ffmpeg")]
    {
        crate::output::sdk_ffmpeg_output::probe_video_file(video_path)
    }
    #[cfg(not(feature = "ffmpeg"))]
    {
        anyhow::bail!("ffmpeg feature is required for video file probing");
    }
}

pub fn extract_preview_frame(
    video_path: &Path,
    timestamp_secs: f64,
    max_width: u32,
) -> Result<RgbaImage> {
    #[cfg(feature = "ffmpeg")]
    {
        crate::output::sdk_ffmpeg_output::extract_preview_frame(
            video_path,
            timestamp_secs,
            max_width,
        )
    }
    #[cfg(not(feature = "ffmpeg"))]
    {
        anyhow::bail!("ffmpeg feature is required for preview frame extraction");
    }
}

pub fn spawn_clip_export(
    request: ClipExportRequest,
    progress_tx: Sender<ClipExportUpdate>,
    cancel_flag: Arc<AtomicBool>,
) {
    thread::spawn(move || {
        let result = run_clip_export(&request, &progress_tx, &cancel_flag);
        match result {
            Ok(ExportOutcome::Finished(path)) => {
                let _ = progress_tx.send(ClipExportUpdate::Finished(path));
            }
            Ok(ExportOutcome::Cancelled) => {
                let _ = progress_tx.send(ClipExportUpdate::Cancelled);
            }
            Err(err) => {
                error!(
                    input_path = ?request.input_path,
                    output_path = ?request.output_path,
                    target_size_mb = request.target_size_mb,
                    "Clip export failed: {err:#}"
                );
                let _ = progress_tx.send(ClipExportUpdate::Failed(format!("{err:#}")));
            }
        }
    });
}
fn run_clip_export(
    request: &ClipExportRequest,
    progress_tx: &Sender<ClipExportUpdate>,
    cancel_flag: &Arc<AtomicBool>,
) -> Result<ExportOutcome> {
    if request.keep_ranges.is_empty() {
        bail!("Cannot export a clip with no kept ranges");
    }

    std::fs::create_dir_all(
        request
            .output_path
            .parent()
            .context("Output path is missing a parent directory")?,
    )
    .with_context(|| {
        format!(
            "Failed to create output directory for {:?}",
            request.output_path
        )
    })?;

    if request.stream_copy && request.crop.is_none() {
        #[cfg(feature = "ffmpeg")]
        {
            return super::sdk_export::run_stream_copy_export_sdk(
                request,
                progress_tx,
                cancel_flag,
            );
        }
        #[cfg(not(feature = "ffmpeg"))]
        {
            anyhow::bail!("ffmpeg feature is required for stream copy export");
        }
    }

    let export_started_at = Instant::now();
    let output_duration_secs = request.output_duration_secs().max(0.1);
    let output_complexity_ratio = export_output_complexity_ratio(request);
    let (output_width, output_height) = resolved_output_dimensions(request);
    let output_fps = resolved_output_fps_for_request(request);

    info!(
        input = ?request.input_path,
        output = ?request.output_path,
        target_size_mb = request.target_size_mb,
        output_duration_secs,
        output_width,
        output_height,
        output_fps = format!("{:.2}", output_fps),
        output_complexity_ratio = format!("{:.4}", output_complexity_ratio),
        "Starting clip export"
    );

    let export_work_dir = std::env::temp_dir().join(format!(
        "liteclip-export-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_millis()
    ));
    std::fs::create_dir_all(&export_work_dir).with_context(|| {
        format!(
            "Failed to create temporary export directory {:?}",
            export_work_dir
        )
    })?;
    let _work_dir_guard = WorkDirGuard::new(export_work_dir.clone());

    let mut selected_encoder = select_export_video_encoder(request)
        .with_context(|| "Unable to resolve an export video encoder")?;

    loop {
        if cancel_flag.load(Ordering::Relaxed) {
            return Ok(ExportOutcome::Cancelled);
        }

        let budget = SizeBudget::from_request(request, selected_encoder);
        let initial_video_bitrate_kbps = match calibrate_initial_bitrate(
            request,
            &budget,
            selected_encoder,
            &export_work_dir,
            progress_tx,
            cancel_flag,
        ) {
            Ok(Some(video_kbps)) => video_kbps,
            Ok(None) => return Ok(ExportOutcome::Cancelled),
            Err(err)
                if selected_encoder != ExportVideoEncoder::SoftwareHevc
                    && should_fallback_to_software_encoder(&err) =>
            {
                warn!(
                    encoder = selected_encoder.ffmpeg_name(),
                    "Calibration failed, falling back to software export encoder: {err:#}"
                );
                selected_encoder = ExportVideoEncoder::SoftwareHevc;
                continue;
            }
            Err(err) => return Err(err),
        };

        match run_bitrate_search(
            request,
            &budget,
            selected_encoder,
            initial_video_bitrate_kbps,
            &export_work_dir,
            progress_tx,
            cancel_flag,
        ) {
            Ok(SearchOutcome::Selected(selected_attempt)) => {
                move_or_copy_file(&selected_attempt.output_path, &request.output_path)
                    .with_context(|| {
                        format!(
                            "Failed to move export output from {:?} to {:?}",
                            selected_attempt.output_path, request.output_path
                        )
                    })?;

                #[cfg(feature = "ffmpeg")]
                {
                    match probe_video_file(&request.output_path) {
                        Ok(output_metadata) => {
                            let violations = validate_export_validity(ExportValidationInput {
                                expected_duration_secs: output_duration_secs,
                                expect_audio: request.metadata.has_audio,
                                metadata: &output_metadata,
                            });
                            if !violations.is_empty() {
                                let details = violations
                                    .into_iter()
                                    .map(|violation| violation.to_string())
                                    .collect::<Vec<_>>()
                                    .join("; ");
                                warn!(
                                    "Export output quality contract warnings for {:?}: {}",
                                    request.output_path, details
                                );
                            }
                        }
                        Err(err) => {
                            warn!(
                                "Failed to probe exported output for validity checks: {:?}: {}",
                                request.output_path, err
                            );
                        }
                    }
                }

                info!(
                    elapsed_secs = export_started_at.elapsed().as_secs_f64(),
                    final_size_bytes = selected_attempt.size_bytes,
                    target_size_bytes = budget.target_size_bytes,
                    final_encoder = selected_encoder.ffmpeg_name(),
                    "Clip export completed"
                );
                return Ok(ExportOutcome::Finished(request.output_path.clone()));
            }
            Ok(SearchOutcome::Cancelled) => return Ok(ExportOutcome::Cancelled),
            Err(err)
                if selected_encoder != ExportVideoEncoder::SoftwareHevc
                    && should_fallback_to_software_encoder(&err) =>
            {
                warn!(
                    encoder = selected_encoder.ffmpeg_name(),
                    "Export attempt failed, falling back to software export encoder: {err:#}"
                );
                selected_encoder = ExportVideoEncoder::SoftwareHevc;
            }
            Err(err) => return Err(err),
        }
    }
}

enum SearchOutcome {
    Selected(ExportAttemptResult),
    Cancelled,
}

fn calibrate_initial_bitrate(
    request: &ClipExportRequest,
    budget: &SizeBudget,
    encoder: ExportVideoEncoder,
    export_work_dir: &Path,
    progress_tx: &Sender<ClipExportUpdate>,
    cancel_flag: &Arc<AtomicBool>,
) -> Result<Option<u32>> {
    if request.output_duration_secs() < CALIBRATION_MIN_CLIP_DURATION_SECS {
        return Ok(Some(budget.initial_video_bitrate_kbps));
    }

    let sample_request =
        build_calibration_sample_request(request, export_work_dir.join("calibration-sample.mp4"));
    let sample_duration_secs = sample_request.output_duration_secs().max(0.1);
    let sample_non_video_bytes = estimate_non_video_bytes(
        sample_duration_secs,
        budget.audio_bitrate_kbps,
        sample_request.keep_ranges.len(),
    );

    let low_bitrate_kbps =
        ((budget.initial_video_bitrate_kbps as f64) * CALIBRATION_LOW_FACTOR).round() as u32;
    let low_bitrate_kbps = low_bitrate_kbps.max(encoder.min_video_bitrate_kbps());
    let mut high_bitrate_kbps =
        ((budget.initial_video_bitrate_kbps as f64) * CALIBRATION_HIGH_FACTOR).round() as u32;
    high_bitrate_kbps = high_bitrate_kbps
        .max(low_bitrate_kbps.saturating_add(150))
        .min(MAX_VIDEO_BITRATE_KBPS);

    info!(
        encoder = encoder.ffmpeg_name(),
        low_bitrate_kbps,
        high_bitrate_kbps,
        sample_duration_secs = format!("{:.2}", sample_duration_secs),
        "Starting export calibration"
    );

    let low_result = match super::sdk_export::attempt_export(
        &sample_request,
        &export_work_dir.join("cal-low.mp4"),
        low_bitrate_kbps,
        budget.audio_bitrate_kbps,
        encoder,
        progress_tx,
        cancel_flag,
        0,
        2,
        ClipExportPhase::Calibration,
    )? {
        Some(result) => result,
        None => return Ok(None),
    };

    // Clean up low calibration file immediately to free disk space
    let _ = std::fs::remove_file(export_work_dir.join("cal-low.mp4"));

    // Hint to the OS that we've finished a major allocation phase
    // This encourages release of FFmpeg's internal memory pools
    std::mem::drop(std::vec::Vec::<u8>::with_capacity(0));

    let high_result = match super::sdk_export::attempt_export(
        &sample_request,
        &export_work_dir.join("cal-high.mp4"),
        high_bitrate_kbps,
        budget.audio_bitrate_kbps,
        encoder,
        progress_tx,
        cancel_flag,
        1,
        2,
        ClipExportPhase::Calibration,
    )? {
        Some(result) => result,
        None => return Ok(None),
    };

    // Clean up high calibration file immediately
    let _ = std::fs::remove_file(export_work_dir.join("cal-high.mp4"));

    let low_point = CalibrationPoint {
        video_bitrate_kbps: low_bitrate_kbps,
        total_output_bytes: low_result.size_bytes,
        sample_duration_secs,
        sample_segments: sample_request.keep_ranges.len(),
    };
    let high_point = CalibrationPoint {
        video_bitrate_kbps: high_bitrate_kbps,
        total_output_bytes: high_result.size_bytes,
        sample_duration_secs,
        sample_segments: sample_request.keep_ranges.len(),
    };

    let target_video_bytes_per_sec = (budget.target_video_bytes as f64
        * CALIBRATION_TARGET_FILL_RATIO)
        / budget.output_duration_secs.max(0.1);
    let low_video_bytes_per_sec = observed_video_bytes_per_sec(
        low_point.total_output_bytes,
        sample_non_video_bytes,
        low_point.sample_duration_secs,
    );
    let high_video_bytes_per_sec = observed_video_bytes_per_sec(
        high_point.total_output_bytes,
        sample_non_video_bytes,
        high_point.sample_duration_secs,
    );

    let calibrated = solve_power_bitrate(
        low_point.video_bitrate_kbps,
        low_video_bytes_per_sec,
        high_point.video_bitrate_kbps,
        high_video_bytes_per_sec,
        target_video_bytes_per_sec.max(1.0),
        encoder.min_video_bitrate_kbps(),
        MAX_VIDEO_BITRATE_KBPS,
    )
    .unwrap_or(budget.initial_video_bitrate_kbps);

    info!(
        encoder = encoder.ffmpeg_name(),
        calibrated_video_bitrate_kbps = calibrated,
        low_output_bytes = low_result.size_bytes,
        high_output_bytes = high_result.size_bytes,
        sample_segments = low_point.sample_segments,
        "Calibration completed"
    );

    Ok(Some(calibrated))
}

fn run_bitrate_search(
    request: &ClipExportRequest,
    budget: &SizeBudget,
    encoder: ExportVideoEncoder,
    initial_video_bitrate_kbps: u32,
    export_work_dir: &Path,
    progress_tx: &Sender<ClipExportUpdate>,
    cancel_flag: &Arc<AtomicBool>,
) -> Result<SearchOutcome> {
    let mut current_video_bitrate_kbps =
        initial_video_bitrate_kbps.max(encoder.min_video_bitrate_kbps());
    let mut best_under_target: Option<ExportAttemptResult> = None;
    let mut smallest_over_target: Option<ExportAttemptResult> = None;

    for attempt_index in 0..MAX_EXPORT_ATTEMPTS {
        if cancel_flag.load(Ordering::Relaxed) {
            return Ok(SearchOutcome::Cancelled);
        }

        let output_path = export_work_dir.join(format!("attempt-{}.mp4", attempt_index + 1));
        let attempt_started_at = Instant::now();
        let attempt_result = match super::sdk_export::attempt_export(
            request,
            &output_path,
            current_video_bitrate_kbps,
            budget.audio_bitrate_kbps,
            encoder,
            progress_tx,
            cancel_flag,
            attempt_index,
            MAX_EXPORT_ATTEMPTS,
            ClipExportPhase::SecondPass,
        )? {
            Some(result) => result,
            None => return Ok(SearchOutcome::Cancelled),
        };

        let fill_ratio = attempt_result.size_bytes as f64 / budget.target_size_bytes as f64;
        info!(
            attempt_index = attempt_index + 1,
            encoder = encoder.ffmpeg_name(),
            video_bitrate_kbps = current_video_bitrate_kbps,
            size_bytes = attempt_result.size_bytes,
            fill_ratio = format!("{:.4}", fill_ratio),
            attempt_elapsed_secs = attempt_started_at.elapsed().as_secs_f64(),
            "Completed export attempt"
        );

        if attempt_result.size_bytes <= budget.target_size_bytes {
            if best_under_target
                .as_ref()
                .map(|best| attempt_result.size_bytes > best.size_bytes)
                .unwrap_or(true)
            {
                // Clean up previous best under target file
                if let Some(ref prev) = best_under_target {
                    let _ = std::fs::remove_file(&prev.output_path);
                }
                best_under_target = Some(attempt_result.clone());
            }
            if fill_ratio >= TARGET_FILL_MIN_RATIO {
                // Clean up smallest over target file if we found a good result
                if let Some(ref over) = smallest_over_target {
                    let _ = std::fs::remove_file(&over.output_path);
                }
                return Ok(SearchOutcome::Selected(attempt_result));
            }
        } else if smallest_over_target
            .as_ref()
            .map(|best| attempt_result.size_bytes < best.size_bytes)
            .unwrap_or(true)
        {
            // Clean up previous smallest over target file
            if let Some(ref prev) = smallest_over_target {
                let _ = std::fs::remove_file(&prev.output_path);
            }
            smallest_over_target = Some(attempt_result.clone());
        } else {
            // This attempt is worse than both best_under and smallest_over, clean it up
            let _ = std::fs::remove_file(&output_path);
        }

        // Hint to release FFmpeg internal memory pools between attempts
        std::mem::drop(std::vec::Vec::<u8>::with_capacity(0));

        let next_bitrate_kbps = next_export_video_bitrate_kbps(
            encoder,
            current_video_bitrate_kbps,
            budget,
            best_under_target.as_ref(),
            smallest_over_target.as_ref(),
        );

        if next_bitrate_kbps == current_video_bitrate_kbps {
            break;
        }
        current_video_bitrate_kbps = next_bitrate_kbps;
    }

    if let Some(best_under_target) = best_under_target.as_ref() {
        let fill_ratio = best_under_target.size_bytes as f64 / budget.target_size_bytes as f64;
        if fill_ratio >= TARGET_FILL_MIN_RATIO {
            return Ok(SearchOutcome::Selected(best_under_target.clone()));
        }
    }

    let best_ratio = best_under_target
        .as_ref()
        .map(|attempt| attempt.size_bytes as f64 / budget.target_size_bytes as f64)
        .unwrap_or(0.0);
    bail!(
        "Unable to produce an export in the {:.0}%..={:.0}% target window without exceeding {} MB. Best safe attempt reached {:.1}% of target.",
        TARGET_FILL_MIN_RATIO * 100.0,
        TARGET_FILL_MAX_RATIO * 100.0,
        request.target_size_mb,
        best_ratio * 100.0
    );
}
pub fn estimate_export_bitrates(
    target_size_mb: u32,
    output_duration_secs: f64,
    has_audio: bool,
    requested_audio_bitrate_kbps: u32,
    num_segments: usize,
    use_hardware_acceleration: bool,
) -> ExportBitrateEstimate {
    let encoder = if use_hardware_acceleration {
        ordered_hardware_encoder_preferences(EncoderType::Auto)
            .into_iter()
            .next()
            .unwrap_or(ExportVideoEncoder::SoftwareHevc)
    } else {
        ExportVideoEncoder::SoftwareHevc
    };
    estimate_export_bitrates_for_encoder(
        target_size_mb,
        output_duration_secs,
        has_audio,
        requested_audio_bitrate_kbps,
        num_segments,
        encoder,
        1.0,
    )
}

fn estimate_export_bitrates_for_encoder(
    target_size_mb: u32,
    output_duration_secs: f64,
    has_audio: bool,
    requested_audio_bitrate_kbps: u32,
    num_segments: usize,
    encoder: ExportVideoEncoder,
    output_complexity_ratio: f64,
) -> ExportBitrateEstimate {
    let duration_secs = output_duration_secs.max(0.1);
    let target_size_bytes = target_size_bytes(target_size_mb);
    let total_kbps = ((target_size_bytes as f64) * 8.0 / duration_secs / 1000.0)
        .max(encoder.min_video_bitrate_kbps() as f64);
    let audio_kbps = select_audio_bitrate_kbps(has_audio, requested_audio_bitrate_kbps, total_kbps);
    let non_video_bytes = estimate_non_video_bytes(duration_secs, audio_kbps, num_segments);
    let complexity_safety = if output_complexity_ratio > 1.0 {
        0.94
    } else if output_complexity_ratio < 0.5 {
        0.98
    } else {
        INITIAL_TARGET_FILL_RATIO
    };
    let target_video_bytes = ((target_size_bytes as f64) * complexity_safety).round() as u64;
    let target_video_bytes = target_video_bytes.saturating_sub(non_video_bytes).max(1);
    let theoretical_video_kbps = ((target_video_bytes as f64) * 8.0 / duration_secs / 1000.0)
        .max(encoder.min_video_bitrate_kbps() as f64);

    ExportBitrateEstimate {
        video_kbps: theoretical_video_kbps.round() as u32,
        audio_kbps,
        total_kbps: total_kbps.round() as u32,
    }
}

fn target_size_bytes(target_size_mb: u32) -> u64 {
    u64::from(target_size_mb).saturating_mul(1024 * 1024)
}

fn estimate_non_video_bytes(
    output_duration_secs: f64,
    audio_bitrate_kbps: u32,
    num_segments: usize,
) -> u64 {
    let duration_secs = output_duration_secs.max(0.0);
    let audio_bytes = (duration_secs * f64::from(audio_bitrate_kbps) * 1000.0 / 8.0).round() as u64;
    audio_bytes.saturating_add(estimate_container_overhead_bytes(
        output_duration_secs,
        num_segments,
    ))
}

fn estimate_container_overhead_bytes(output_duration_secs: f64, num_segments: usize) -> u64 {
    let stream_bytes = (output_duration_secs.max(0.0)
        * (8.0 + (num_segments as f64 * 3.0))
        * 1000.0
        / 8.0)
        .round() as u64;
    32 * 1024 + stream_bytes
}

fn select_audio_bitrate_kbps(
    has_audio: bool,
    requested_audio_bitrate_kbps: u32,
    total_bitrate_kbps: f64,
) -> u32 {
    if !has_audio {
        return 0;
    }

    let requested_audio_bitrate_kbps = requested_audio_bitrate_kbps.max(48);
    let adaptive_cap = if total_bitrate_kbps < 700.0 {
        48
    } else if total_bitrate_kbps < 1100.0 {
        64
    } else if total_bitrate_kbps < 1800.0 {
        80
    } else if total_bitrate_kbps < 2600.0 {
        96
    } else {
        128
    };
    let share_cap = (total_bitrate_kbps * 0.2).round().clamp(48.0, 160.0) as u32;
    requested_audio_bitrate_kbps
        .min(adaptive_cap)
        .min(share_cap)
}

fn build_calibration_sample_request(
    request: &ClipExportRequest,
    output_path: PathBuf,
) -> ClipExportRequest {
    let full_duration_secs = request.output_duration_secs().max(0.1);
    let sample_duration_secs = (full_duration_secs * CALIBRATION_SAMPLE_RATIO)
        .clamp(CALIBRATION_MIN_SAMPLE_SECS, CALIBRATION_MAX_SAMPLE_SECS)
        .min(full_duration_secs);

    let window_count = if sample_duration_secs >= 6.0 {
        3
    } else if sample_duration_secs >= 2.0 {
        2
    } else {
        1
    };
    let window_duration_secs = (sample_duration_secs / window_count as f64).max(0.5);
    let max_window_start = (full_duration_secs - window_duration_secs).max(0.0);

    let window_starts: Vec<f64> = if window_count == 1 {
        vec![max_window_start / 2.0]
    } else {
        (0..window_count)
            .map(|index| max_window_start * index as f64 / (window_count - 1) as f64)
            .collect()
    };

    let mut keep_ranges = Vec::new();
    for window_start_secs in window_starts {
        keep_ranges.extend(slice_keep_ranges_for_output_window(
            &request.keep_ranges,
            window_start_secs,
            window_duration_secs,
        ));
    }

    let mut sample_request = request.clone();
    sample_request.output_path = output_path;
    sample_request.keep_ranges = keep_ranges;
    sample_request
}

fn slice_keep_ranges_for_output_window(
    keep_ranges: &[TimeRange],
    window_start_secs: f64,
    window_duration_secs: f64,
) -> Vec<TimeRange> {
    let mut sliced = Vec::new();
    let window_start_secs = window_start_secs.max(0.0);
    let window_end_secs =
        (window_start_secs + window_duration_secs.max(0.0)).max(window_start_secs);
    let mut output_cursor = 0.0;

    for range in keep_ranges {
        let range_duration = range.duration_secs();
        let range_start = output_cursor;
        let range_end = output_cursor + range_duration;

        if range_end <= window_start_secs {
            output_cursor = range_end;
            continue;
        }
        if range_start >= window_end_secs {
            break;
        }

        let overlap_start = window_start_secs.max(range_start);
        let overlap_end = window_end_secs.min(range_end);
        if overlap_end > overlap_start {
            sliced.push(TimeRange {
                start_secs: range.start_secs + (overlap_start - range_start),
                end_secs: range.start_secs + (overlap_end - range_start),
            });
        }

        output_cursor = range_end;
    }

    sliced
}

fn observed_video_bytes(total_size_bytes: u64, estimated_non_video_bytes: u64) -> f64 {
    total_size_bytes
        .saturating_sub(estimated_non_video_bytes)
        .max(1) as f64
}

fn observed_video_bytes_per_sec(
    total_size_bytes: u64,
    estimated_non_video_bytes: u64,
    duration_secs: f64,
) -> f64 {
    observed_video_bytes(total_size_bytes, estimated_non_video_bytes) / duration_secs.max(0.1)
}

fn next_export_video_bitrate_kbps(
    encoder: ExportVideoEncoder,
    current_video_bitrate_kbps: u32,
    budget: &SizeBudget,
    best_under_target: Option<&ExportAttemptResult>,
    smallest_over_target: Option<&ExportAttemptResult>,
) -> u32 {
    let target_video_bytes = ((budget.target_video_bytes as f64) * RETRY_TARGET_FILL_RATIO).round();
    let min_video_bitrate_kbps = encoder.min_video_bitrate_kbps();

    let mut next_video_bitrate_kbps = match (best_under_target, smallest_over_target) {
        (Some(under), Some(over)) => {
            let under_video_bytes =
                observed_video_bytes(under.size_bytes, budget.estimated_non_video_bytes);
            let over_video_bytes =
                observed_video_bytes(over.size_bytes, budget.estimated_non_video_bytes);
            solve_power_bitrate(
                under.video_bitrate_kbps,
                under_video_bytes,
                over.video_bitrate_kbps,
                over_video_bytes,
                target_video_bytes.max(1.0),
                under.video_bitrate_kbps.min(over.video_bitrate_kbps),
                under.video_bitrate_kbps.max(over.video_bitrate_kbps),
            )
            .unwrap_or_else(|| {
                under.video_bitrate_kbps
                    + (over
                        .video_bitrate_kbps
                        .saturating_sub(under.video_bitrate_kbps)
                        / 2)
            })
        }
        (None, Some(over)) => {
            let over_video_bytes =
                observed_video_bytes(over.size_bytes, budget.estimated_non_video_bytes);
            let ratio = (target_video_bytes / over_video_bytes).clamp(0.05, 0.98);
            ((over.video_bitrate_kbps as f64) * ratio.powf(0.95)).round() as u32
        }
        (Some(under), None) => {
            let under_video_bytes =
                observed_video_bytes(under.size_bytes, budget.estimated_non_video_bytes);
            let ratio = (target_video_bytes / under_video_bytes).clamp(1.01, 2.0);
            ((under.video_bitrate_kbps as f64) * ratio.powf(0.9)).round() as u32
        }
        (None, None) => current_video_bitrate_kbps,
    };

    if let Some(under) = best_under_target {
        if next_video_bitrate_kbps <= under.video_bitrate_kbps {
            next_video_bitrate_kbps = under.video_bitrate_kbps.saturating_add(BITRATE_NUDGE_KBPS);
        }
    }
    if let Some(over) = smallest_over_target {
        if next_video_bitrate_kbps >= over.video_bitrate_kbps {
            next_video_bitrate_kbps = over.video_bitrate_kbps.saturating_sub(BITRATE_NUDGE_KBPS);
        }
    }

    if next_video_bitrate_kbps == current_video_bitrate_kbps {
        if smallest_over_target.is_some() {
            next_video_bitrate_kbps = current_video_bitrate_kbps.saturating_sub(BITRATE_NUDGE_KBPS);
        } else if best_under_target.is_some() {
            next_video_bitrate_kbps = current_video_bitrate_kbps.saturating_add(BITRATE_NUDGE_KBPS);
        }
    }

    next_video_bitrate_kbps.clamp(min_video_bitrate_kbps, MAX_VIDEO_BITRATE_KBPS)
}

fn solve_power_bitrate(
    first_bitrate_kbps: u32,
    first_value: f64,
    second_bitrate_kbps: u32,
    second_value: f64,
    target_value: f64,
    min_bitrate_kbps: u32,
    max_bitrate_kbps: u32,
) -> Option<u32> {
    let first_bitrate = first_bitrate_kbps as f64;
    let second_bitrate = second_bitrate_kbps as f64;

    if first_bitrate <= 0.0
        || second_bitrate <= 0.0
        || first_value <= 0.0
        || second_value <= 0.0
        || target_value <= 0.0
        || (first_bitrate - second_bitrate).abs() < f64::EPSILON
    {
        return None;
    }

    let exponent = ((second_value / first_value).ln() / (second_bitrate / first_bitrate).ln())
        .clamp(
            CALIBRATION_POWER_EXPONENT_MIN,
            CALIBRATION_POWER_EXPONENT_MAX,
        );
    if !exponent.is_finite() {
        return None;
    }

    let coefficient = first_value / first_bitrate.powf(exponent);
    if !coefficient.is_finite() || coefficient <= 0.0 {
        return None;
    }

    let bitrate_kbps = (target_value / coefficient).powf(1.0 / exponent);
    if !bitrate_kbps.is_finite() {
        return None;
    }

    Some(
        bitrate_kbps
            .round()
            .clamp(min_bitrate_kbps as f64, max_bitrate_kbps as f64) as u32,
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExportVideoEncoder {
    SoftwareHevc,
    HevcNvenc,
    HevcAmf,
    HevcQsv,
}

impl ExportVideoEncoder {
    pub fn ffmpeg_name(self) -> &'static str {
        match self {
            ExportVideoEncoder::SoftwareHevc => "libx265",
            ExportVideoEncoder::HevcNvenc => "hevc_nvenc",
            ExportVideoEncoder::HevcAmf => "hevc_amf",
            ExportVideoEncoder::HevcQsv => "hevc_qsv",
        }
    }

    pub fn supports_two_pass(self) -> bool {
        false
    }

    fn min_video_bitrate_kbps(self) -> u32 {
        MIN_VIDEO_BITRATE_KBPS
    }
}
fn ordered_hardware_encoder_preferences(preferred: EncoderType) -> Vec<ExportVideoEncoder> {
    let mut order = Vec::with_capacity(3);
    match preferred {
        EncoderType::Nvenc => order.push(ExportVideoEncoder::HevcNvenc),
        EncoderType::Amf => order.push(ExportVideoEncoder::HevcAmf),
        EncoderType::Qsv => order.push(ExportVideoEncoder::HevcQsv),
        EncoderType::Software | EncoderType::Auto => {}
    }

    for encoder in [
        ExportVideoEncoder::HevcNvenc,
        ExportVideoEncoder::HevcAmf,
        ExportVideoEncoder::HevcQsv,
    ] {
        if !order.contains(&encoder) {
            order.push(encoder);
        }
    }
    order
}

fn export_encoder_type(encoder: ExportVideoEncoder) -> EncoderType {
    match encoder {
        ExportVideoEncoder::SoftwareHevc => EncoderType::Software,
        ExportVideoEncoder::HevcNvenc => EncoderType::Nvenc,
        ExportVideoEncoder::HevcAmf => EncoderType::Amf,
        ExportVideoEncoder::HevcQsv => EncoderType::Qsv,
    }
}

fn export_encoder_is_available(encoder: ExportVideoEncoder) -> bool {
    if encoder == ExportVideoEncoder::SoftwareHevc {
        return true;
    }

    #[cfg(not(feature = "ffmpeg"))]
    {
        let _ = encoder;
        return false;
    }

    let config = EncoderConfig::new(8, 60, (1280, 720), export_encoder_type(encoder), 2);
    resolve_effective_encoder_config(&config).is_ok()
}

fn select_export_video_encoder(request: &ClipExportRequest) -> Result<ExportVideoEncoder> {
    if !request.use_hardware_acceleration {
        return Ok(ExportVideoEncoder::SoftwareHevc);
    }

    for encoder in ordered_hardware_encoder_preferences(request.preferred_encoder) {
        if export_encoder_is_available(encoder) {
            return Ok(encoder);
        }
    }

    Ok(ExportVideoEncoder::SoftwareHevc)
}

fn should_fallback_to_software_encoder(err: &anyhow::Error) -> bool {
    let message = format!("{err:#}").to_lowercase();
    [
        "cannot load nvcuda.dll",
        "no capable devices found",
        "device not available",
        "driver does not support required nvenc",
        "hevc_nvenc",
        "hevc_amf",
        "hevc_qsv",
        "error while opening encoder",
        "initializing an internal mfx session failed",
        "amf initialization",
    ]
    .iter()
    .any(|needle| message.contains(needle))
}

fn move_or_copy_file(source_path: &Path, destination_path: &Path) -> Result<()> {
    let _ = std::fs::remove_file(destination_path);
    match std::fs::rename(source_path, destination_path) {
        Ok(()) => Ok(()),
        Err(_) => {
            std::fs::copy(source_path, destination_path).with_context(|| {
                format!(
                    "Failed to copy export output from {:?} to {:?}",
                    source_path, destination_path
                )
            })?;
            let _ = std::fs::remove_file(source_path);
            Ok(())
        }
    }
}

fn cleanup_export_work_dir(work_dir: &Path) {
    let _ = std::fs::remove_dir_all(work_dir);
}

struct WorkDirGuard {
    path: Option<PathBuf>,
}

impl WorkDirGuard {
    fn new(path: PathBuf) -> Self {
        Self { path: Some(path) }
    }
}

impl Drop for WorkDirGuard {
    fn drop(&mut self) {
        if let Some(path) = self.path.take() {
            cleanup_export_work_dir(&path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_request() -> ClipExportRequest {
        ClipExportRequest {
            input_path: PathBuf::from("input.mp4"),
            output_path: PathBuf::from("output.mp4"),
            keep_ranges: vec![TimeRange {
                start_secs: 0.0,
                end_secs: 120.0,
            }],
            target_size_mb: 8,
            audio_bitrate_kbps: 128,
            use_hardware_acceleration: true,
            preferred_encoder: EncoderType::Auto,
            metadata: VideoFileMetadata {
                duration_secs: 120.0,
                width: 2560,
                height: 1440,
                has_audio: true,
                fps: 60.0,
            },
            stream_copy: false,
            output_width: None,
            output_height: None,
            output_fps: None,
            crop: None,
            post_process_filters: true,
        }
    }

    #[test]
    fn estimate_export_bitrates_scales_audio_down_for_small_budgets() {
        let estimate = estimate_export_bitrates(1, 20.0, true, 128, 2, false);
        assert_eq!(estimate.audio_kbps, 48);
        assert!(estimate.video_kbps >= MIN_VIDEO_BITRATE_KBPS);
        assert!(estimate.total_kbps >= estimate.video_kbps);
    }

    #[test]
    fn calibration_sample_spreads_windows_for_single_range_clip() {
        let request = base_request();
        let sample = build_calibration_sample_request(&request, PathBuf::from("cal.mp4"));

        assert_eq!(sample.keep_ranges.len(), 3);
        assert!(sample.keep_ranges[0].start_secs < 1.0);
        assert!(sample.keep_ranges[1].start_secs > 50.0);
        assert!(sample.keep_ranges[2].end_secs > 119.0);
    }

    #[test]
    fn calibration_sample_slices_across_multi_range_clip() {
        let mut request = base_request();
        request.keep_ranges = vec![
            TimeRange {
                start_secs: 0.0,
                end_secs: 5.0,
            },
            TimeRange {
                start_secs: 10.0,
                end_secs: 20.0,
            },
        ];

        let sample = build_calibration_sample_request(&request, PathBuf::from("cal.mp4"));
        assert!(!sample.keep_ranges.is_empty());
        assert!(sample.output_duration_secs() <= CALIBRATION_MAX_SAMPLE_SECS);
    }

    #[test]
    fn solve_power_bitrate_matches_linear_curve() {
        let solved = solve_power_bitrate(200, 25_000.0, 400, 50_000.0, 37_500.0, 100, 1_000)
            .expect("solve should succeed");
        assert!((295..=305).contains(&solved), "solved={solved}");
    }

    #[test]
    fn next_bitrate_moves_inside_bracket() {
        let budget = SizeBudget {
            target_size_bytes: target_size_bytes(8),
            output_duration_secs: 30.0,
            audio_bitrate_kbps: 96,
            estimated_non_video_bytes: 100_000,
            target_video_bytes: target_size_bytes(8) - 100_000,
            initial_video_bitrate_kbps: 2_000,
        };
        let under = ExportAttemptResult {
            output_path: PathBuf::from("under.mp4"),
            video_bitrate_kbps: 1800,
            size_bytes: ((budget.target_size_bytes as f64) * 0.91).round() as u64,
        };
        let over = ExportAttemptResult {
            output_path: PathBuf::from("over.mp4"),
            video_bitrate_kbps: 2200,
            size_bytes: ((budget.target_size_bytes as f64) * 1.03).round() as u64,
        };

        let next = next_export_video_bitrate_kbps(
            ExportVideoEncoder::SoftwareHevc,
            2000,
            &budget,
            Some(&under),
            Some(&over),
        );
        assert!(next > under.video_bitrate_kbps);
        assert!(next < over.video_bitrate_kbps);
    }

    #[test]
    fn auto_export_hardware_order_matches_runtime_detection_priority() {
        assert_eq!(
            ordered_hardware_encoder_preferences(EncoderType::Auto),
            vec![
                ExportVideoEncoder::HevcNvenc,
                ExportVideoEncoder::HevcAmf,
                ExportVideoEncoder::HevcQsv,
            ]
        );
    }

    #[test]
    fn explicit_preference_is_tried_first_then_uses_common_fallback_order() {
        assert_eq!(
            ordered_hardware_encoder_preferences(EncoderType::Qsv),
            vec![
                ExportVideoEncoder::HevcQsv,
                ExportVideoEncoder::HevcNvenc,
                ExportVideoEncoder::HevcAmf,
            ]
        );
    }

    #[test]
    fn export_complexity_ratio_accounts_for_resolution_fps_and_crop() {
        let mut request = base_request();
        request.output_width = Some(2048);
        request.output_height = Some(1152);
        request.output_fps = Some(30.0);

        let ratio = export_output_complexity_ratio(&request);
        assert!((ratio - 0.32).abs() < 0.01, "ratio={ratio}");
    }

    #[test]
    fn budget_reserves_non_video_bytes_and_targets_under_cap() {
        let request = base_request();
        let budget = SizeBudget::from_request(&request, ExportVideoEncoder::SoftwareHevc);

        assert!(budget.estimated_non_video_bytes > 0);
        assert!(budget.target_video_bytes < budget.target_size_bytes);
        assert!(budget.initial_video_bitrate_kbps >= MIN_VIDEO_BITRATE_KBPS);
    }
}
