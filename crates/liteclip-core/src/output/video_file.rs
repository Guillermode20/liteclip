use crate::config::EncoderType;
use anyhow::{bail, Context, Result};
use image::RgbaImage;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::Sender,
    Arc,
};
use std::thread;
use tracing::{error, info, warn};

use std::process::{Command, Stdio};

use super::functions::ffmpeg_executable_path;

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x08000000;

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

/// Webcam companion + layout for burned-in PiP export.
#[derive(Debug, Clone)]
pub struct WebcamExport {
    pub path: PathBuf,
    pub keyframes: Vec<super::webcam_layout::WebcamKeyframe>,
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
    /// Optional second input (companion webcam MP4) and layout for overlay.
    pub webcam: Option<WebcamExport>,
}

impl ClipExportRequest {
    pub fn output_duration_secs(&self) -> f64 {
        self.keep_ranges
            .iter()
            .map(|range| range.duration_secs())
            .sum()
    }
}

enum ExportOutcome {
    Finished(PathBuf),
    Cancelled,
}

#[derive(Debug, Clone, Copy)]
pub struct ExportBitrateEstimate {
    pub video_kbps: u32,
    pub audio_kbps: u32,
    pub total_kbps: u32,
}

const MIN_VIDEO_BITRATE_KBPS: u32 = 300;
const MIN_AUTO_VIDEO_BITRATE_KBPS: u32 = 100;
const MAX_EXPORT_ATTEMPTS: usize = 6;
const AMF_MAX_EXPORT_ATTEMPTS: usize = 2;
const AMF_TARGET_FILL_RATIO_MIN: f64 = 0.90;
const AMF_TARGET_FILL_RATIO_IDEAL: f64 = 0.925;
// AMF vbr_peak with quality preset has a non-linear overshoot ratio: it over-produces by
// a larger percentage at the (higher) calibration bitrate than at the (lower) final bitrate.
// Observed correction factor ~1.056× (1.576 at 376 kbps vs 1.492 at 287 kbps). We target
// a proportionally higher fill ratio in the calibration formula to compensate, so the first
// full encode attempt lands in the acceptable 90-100% window without a retry.
const AMF_CALIBRATION_FILL_RATIO: f64 = 0.990;
const TARGET_SIZE_UNDERFILL_RATIO: f64 = 0.992;
const MAX_VIDEO_BITRATE_KBPS: u32 = 400_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExportVideoEncoder {
    SoftwareHevc,
    HevcNvenc,
    HevcAmf,
    HevcQsv,
}

impl ExportVideoEncoder {
    fn ffmpeg_name(self) -> &'static str {
        match self {
            ExportVideoEncoder::SoftwareHevc => "libx265",
            ExportVideoEncoder::HevcNvenc => "hevc_nvenc",
            ExportVideoEncoder::HevcAmf => "hevc_amf",
            ExportVideoEncoder::HevcQsv => "hevc_qsv",
        }
    }

    fn supports_two_pass(self) -> bool {
        matches!(self, ExportVideoEncoder::SoftwareHevc)
    }

    fn initial_target_fill_ratio(self) -> f64 {
        match self {
            ExportVideoEncoder::HevcAmf => AMF_TARGET_FILL_RATIO_IDEAL,
            _ => 1.0,
        }
    }

    fn acceptable_fill_range(self) -> (f64, f64) {
        match self {
            ExportVideoEncoder::HevcAmf => (AMF_TARGET_FILL_RATIO_MIN, 1.0),
            _ => (TARGET_SIZE_UNDERFILL_RATIO, 1.0),
        }
    }

    fn max_attempts(self) -> usize {
        match self {
            ExportVideoEncoder::HevcAmf => AMF_MAX_EXPORT_ATTEMPTS,
            _ => MAX_EXPORT_ATTEMPTS,
        }
    }

    fn prefers_direct_retry(self) -> bool {
        matches!(self, ExportVideoEncoder::HevcAmf)
    }

    fn min_video_bitrate_kbps(self) -> u32 {
        match self {
            ExportVideoEncoder::HevcAmf => MIN_AUTO_VIDEO_BITRATE_KBPS,
            _ => MIN_VIDEO_BITRATE_KBPS,
        }
    }

    fn bitrate_efficiency_factor(self) -> f64 {
        match self {
            ExportVideoEncoder::SoftwareHevc => 0.85,
            // AMF HEVC tracks the requested bitrate closely enough that a near-theoretical
            // first-pass estimate lands in the target band more reliably than the old
            // generic hardware discount.
            ExportVideoEncoder::HevcAmf => 0.96,
            ExportVideoEncoder::HevcNvenc | ExportVideoEncoder::HevcQsv => 0.50,
        }
    }
}

pub fn probe_video_file(video_path: &Path) -> Result<VideoFileMetadata> {
    #[cfg(feature = "ffmpeg")]
    {
        return crate::output::sdk_ffmpeg_output::probe_video_file(video_path);
    }
    #[cfg(all(feature = "ffmpeg-cli", not(feature = "ffmpeg")))]
    {
        return probe_video_file_ffprobe(video_path);
    }
    #[cfg(not(any(feature = "ffmpeg", feature = "ffmpeg-cli")))]
    {
        anyhow::bail!("no FFmpeg backend enabled; use --features ffmpeg or ffmpeg-cli");
    }
}

#[cfg(all(feature = "ffmpeg-cli", not(feature = "ffmpeg")))]
fn probe_video_file_ffprobe(video_path: &Path) -> Result<VideoFileMetadata> {
    let ffprobe = ffprobe_executable_path();
    let output = command_output(Command::new(&ffprobe).args([
        "-v",
        "error",
        "-show_entries",
        "format=duration:stream=codec_type,width,height,r_frame_rate",
        "-of",
        "default=noprint_wrappers=1:nokey=0",
        &video_path.to_string_lossy(),
    ]))
    .with_context(|| format!("Failed to probe video metadata via {:?}", ffprobe))?;

    if !output.status.success() {
        bail!(
            "ffprobe failed for {:?}: {}",
            video_path,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut current_stream = String::new();
    let mut duration_secs = None;
    let mut width = None;
    let mut height = None;
    let mut has_audio = false;
    let mut fps = None;

    for line in stdout.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };

        match key.trim() {
            "codec_type" => {
                current_stream = value.trim().to_string();
                has_audio |= current_stream == "audio";
            }
            "width" if current_stream == "video" && width.is_none() => {
                width = value.trim().parse::<u32>().ok();
            }
            "height" if current_stream == "video" && height.is_none() => {
                height = value.trim().parse::<u32>().ok();
            }
            "r_frame_rate" if current_stream == "video" && fps.is_none() => {
                fps = parse_rational_fps(value.trim());
            }
            "duration" if duration_secs.is_none() => {
                duration_secs = value.trim().parse::<f64>().ok();
            }
            _ => {}
        }
    }

    let duration_secs = duration_secs.context("Video duration was missing from ffprobe output")?;
    let width = width.context("Video width was missing from ffprobe output")?;
    let height = height.context("Video height was missing from ffprobe output")?;

    Ok(VideoFileMetadata {
        duration_secs,
        width,
        height,
        has_audio,
        fps: fps.unwrap_or(30.0),
    })
}

pub fn extract_preview_frame(
    video_path: &Path,
    timestamp_secs: f64,
    max_width: u32,
) -> Result<RgbaImage> {
    #[cfg(feature = "ffmpeg")]
    {
        return crate::output::sdk_ffmpeg_output::extract_preview_frame(
            video_path,
            timestamp_secs,
            max_width,
        );
    }
    #[cfg(all(feature = "ffmpeg-cli", not(feature = "ffmpeg")))]
    {
        return extract_preview_frame_cli(video_path, timestamp_secs, max_width);
    }
    #[cfg(not(any(feature = "ffmpeg", feature = "ffmpeg-cli")))]
    {
        anyhow::bail!("no FFmpeg backend enabled; use --features ffmpeg or ffmpeg-cli");
    }
}

#[cfg(all(feature = "ffmpeg-cli", not(feature = "ffmpeg")))]
fn extract_preview_frame_cli(
    video_path: &Path,
    timestamp_secs: f64,
    max_width: u32,
) -> Result<RgbaImage> {
    let ffmpeg = ffmpeg_executable_path();
    let timestamp = format_seconds_arg(timestamp_secs);
    let scale = format!("scale={max_width}:-2:force_original_aspect_ratio=decrease");

    let output = command_output(
        Command::new(&ffmpeg)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-i",
                &video_path.to_string_lossy(),
                "-ss",
                &timestamp,
                "-frames:v",
                "1",
                "-vf",
                &scale,
                "-f",
                "image2pipe",
                "-vcodec",
                "mjpeg",
                "-",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped()),
    )
    .with_context(|| format!("Failed to extract preview frame via {:?}", ffmpeg))?;

    if !output.status.success() {
        bail!(
            "ffmpeg frame extraction failed for {:?}: {}",
            video_path,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    if output.stdout.is_empty() {
        bail!(
            "ffmpeg returned an empty preview frame for {:?}",
            video_path
        );
    }

    Ok(image::load_from_memory(&output.stdout)
        .context("Failed to decode preview frame image")?
        .into_rgba8())
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

    let output_duration_secs = request.output_duration_secs().max(0.1);
    let mut selected_encoder = select_export_video_encoder(request)
        .with_context(|| "Unable to resolve an export video encoder")?;
    let mut bitrate_estimate = estimate_export_bitrates_for_encoder(
        request.target_size_mb,
        output_duration_secs,
        request.metadata.has_audio,
        request.audio_bitrate_kbps,
        request.keep_ranges.len(),
        selected_encoder,
    );
    let target_size_bytes = target_size_bytes(request.target_size_mb);
    let mut estimated_non_video_bytes = estimate_non_video_bytes(
        output_duration_secs,
        bitrate_estimate.audio_kbps,
        request.keep_ranges.len(),
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

    if selected_encoder == ExportVideoEncoder::HevcAmf {
        let sample_duration_secs = calibration_duration_secs(output_duration_secs);
        let calibration_path = export_work_dir.join("amf-calibration.mp4");
        let calibration_request =
            build_amf_calibration_request(request, sample_duration_secs, calibration_path.clone());
        let calibration_bitrate_kbps = bitrate_estimate
            .video_kbps
            .max(selected_encoder.min_video_bitrate_kbps());

        let mut calibration_result = None;
        match attempt_export(
            &calibration_request,
            &calibration_path,
            calibration_bitrate_kbps,
            bitrate_estimate.audio_kbps,
            selected_encoder,
            progress_tx,
            cancel_flag,
            0,
            2,
            ClipExportPhase::Calibration,
        ) {
            Ok(Some(attempt_result)) => calibration_result = Some(attempt_result),
            Ok(None) => {
                let _ = std::fs::remove_file(&calibration_path);
                return Ok(ExportOutcome::Cancelled);
            }
            Err(err) if should_fallback_to_software_encoder(&err) => {
                warn!(
                    encoder = selected_encoder.ffmpeg_name(),
                    "AMF calibration failed at runtime, falling back to libx265: {err:#}"
                );
                selected_encoder = ExportVideoEncoder::SoftwareHevc;
                bitrate_estimate = estimate_export_bitrates_for_encoder(
                    request.target_size_mb,
                    output_duration_secs,
                    request.metadata.has_audio,
                    request.audio_bitrate_kbps,
                    request.keep_ranges.len(),
                    selected_encoder,
                );
                estimated_non_video_bytes = estimate_non_video_bytes(
                    output_duration_secs,
                    bitrate_estimate.audio_kbps,
                    request.keep_ranges.len(),
                );
                let _ = std::fs::remove_file(&calibration_path);
            }
            Err(err) => {
                let _ = std::fs::remove_file(&calibration_path);
                return Err(err);
            }
        }

        if let Some(calibration_result) = calibration_result {
            let calibrated_video_bitrate_kbps = calibrate_amf_video_bitrate(
                calibration_bitrate_kbps,
                calibration_result.size_bytes,
                calibration_request.output_duration_secs().max(0.1),
                calibration_request.keep_ranges.len(),
                output_duration_secs,
                request.keep_ranges.len(),
                bitrate_estimate.audio_kbps,
                target_size_bytes,
            );
            info!(
                "AMF calibration pass produced {} bytes at {} kbps; next full encode will use {} kbps",
                calibration_result.size_bytes,
                calibration_bitrate_kbps,
                calibrated_video_bitrate_kbps
            );
            bitrate_estimate.video_kbps = calibrated_video_bitrate_kbps;
        }

        let _ = std::fs::remove_file(&calibration_path);
    }

    let export_result = (|| -> Result<ExportOutcome> {
        let mut current_video_bitrate_kbps = bitrate_estimate
            .video_kbps
            .max(selected_encoder.min_video_bitrate_kbps());
        let mut low_video_bitrate_kbps = selected_encoder.min_video_bitrate_kbps();
        let mut high_video_bitrate_kbps: Option<u32> = None;
        let mut best_under_target: Option<ExportAttemptResult> = None;
        let mut best_over_target: Option<ExportAttemptResult> = None;
        let mut amf_attempts = 0usize;

        for attempt_index in 0..MAX_EXPORT_ATTEMPTS {
            if cancel_flag.load(Ordering::Relaxed) {
                return Ok(ExportOutcome::Cancelled);
            }

            let output_path = export_work_dir.join(format!("attempt-{}.mp4", attempt_index + 1));
            let attempt_result = match attempt_export(
                request,
                &output_path,
                current_video_bitrate_kbps,
                bitrate_estimate.audio_kbps,
                selected_encoder,
                progress_tx,
                cancel_flag,
                attempt_index,
                selected_encoder.max_attempts(),
                ClipExportPhase::SecondPass,
            ) {
                Ok(Some(attempt_result)) => attempt_result,
                Ok(None) => return Ok(ExportOutcome::Cancelled),
                Err(err)
                    if selected_encoder != ExportVideoEncoder::SoftwareHevc
                        && should_fallback_to_software_encoder(&err) =>
                {
                    warn!(
                        encoder = selected_encoder.ffmpeg_name(),
                        "Hardware export encoder failed at runtime, falling back to libx265: {err:#}"
                    );
                    selected_encoder = ExportVideoEncoder::SoftwareHevc;
                    bitrate_estimate = estimate_export_bitrates_for_encoder(
                        request.target_size_mb,
                        output_duration_secs,
                        request.metadata.has_audio,
                        request.audio_bitrate_kbps,
                        request.keep_ranges.len(),
                        selected_encoder,
                    );
                    estimated_non_video_bytes = estimate_non_video_bytes(
                        output_duration_secs,
                        bitrate_estimate.audio_kbps,
                        request.keep_ranges.len(),
                    );
                    current_video_bitrate_kbps = bitrate_estimate
                        .video_kbps
                        .max(selected_encoder.min_video_bitrate_kbps());
                    low_video_bitrate_kbps = selected_encoder.min_video_bitrate_kbps();
                    high_video_bitrate_kbps = None;
                    continue;
                }
                Err(err) => return Err(err),
            };

            if selected_encoder == ExportVideoEncoder::HevcAmf {
                amf_attempts = amf_attempts.saturating_add(1);
            }

            info!(
                "Export attempt {}/{} produced {} bytes at {} kbps (target {} bytes)",
                attempt_index + 1,
                selected_encoder.max_attempts(),
                attempt_result.size_bytes,
                attempt_result.video_bitrate_kbps,
                target_size_bytes
            );

            if attempt_result.size_bytes <= target_size_bytes {
                let replace_best_under = should_replace_best_under_attempt(
                    selected_encoder,
                    &attempt_result,
                    best_under_target.as_ref(),
                    target_size_bytes,
                );
                if replace_best_under {
                    best_under_target = Some(attempt_result.clone());
                }
                // Under target: we can try higher bitrates, raise low bound
                low_video_bitrate_kbps = low_video_bitrate_kbps.max(current_video_bitrate_kbps);
            } else {
                let replace_best_over = match best_over_target.as_ref() {
                    Some(best_attempt) => {
                        attempt_result.size_bytes < best_attempt.size_bytes
                            || (attempt_result.size_bytes == best_attempt.size_bytes
                                && attempt_result.video_bitrate_kbps
                                    < best_attempt.video_bitrate_kbps)
                    }
                    None => true,
                };
                if replace_best_over {
                    best_over_target = Some(attempt_result.clone());
                }
                // Over target: need to try lower bitrates, lower the high bound and reduce low bound
                high_video_bitrate_kbps = Some(match high_video_bitrate_kbps {
                    Some(high_bitrate) => high_bitrate.min(current_video_bitrate_kbps),
                    None => current_video_bitrate_kbps,
                });
                // Also reduce low_video_bitrate_kbps to allow going lower on next attempt
                // Don't let it go below absolute minimum of 100 kbps
                low_video_bitrate_kbps = low_video_bitrate_kbps
                    .min(current_video_bitrate_kbps.saturating_sub(10))
                    .max(100);
            }

            let measured_non_video_bytes = estimate_measured_non_video_bytes(
                attempt_result.size_bytes,
                current_video_bitrate_kbps,
                output_duration_secs,
            );
            estimated_non_video_bytes =
                blend_non_video_bytes_estimate(estimated_non_video_bytes, measured_non_video_bytes);

            if attempt_satisfies_target_window(
                selected_encoder,
                attempt_result.size_bytes,
                target_size_bytes,
            ) {
                break;
            }

            let next_video_bitrate_kbps = next_export_video_bitrate_kbps(
                selected_encoder,
                current_video_bitrate_kbps,
                attempt_result.size_bytes,
                target_size_bytes,
                estimated_non_video_bytes,
                low_video_bitrate_kbps,
                high_video_bitrate_kbps,
            );

            if selected_encoder == ExportVideoEncoder::HevcAmf
                && amf_attempts >= AMF_MAX_EXPORT_ATTEMPTS
            {
                break;
            }

            // Always try to adjust bitrate if over target, even if calculation returns same value
            // The actual output was wrong, so we should retry with adjusted parameters
            if next_video_bitrate_kbps == current_video_bitrate_kbps
                && attempt_result.size_bytes < target_size_bytes
            {
                break;
            }

            if let Some(high_bitrate) = high_video_bitrate_kbps {
                // Only exit early if we're under target and converging, not if we're over target
                if attempt_result.size_bytes <= target_size_bytes
                    && high_bitrate <= low_video_bitrate_kbps.saturating_add(24)
                {
                    break;
                }
            }

            current_video_bitrate_kbps = next_video_bitrate_kbps;
        }

        let preferred_attempt =
            select_preferred_attempt(best_under_target, best_over_target, target_size_bytes, true)
                .context("Clip export did not produce an output file")?;

        move_or_copy_file(&preferred_attempt.output_path, &request.output_path).with_context(
            || {
                format!(
                    "Failed to move export output from {:?} to {:?}",
                    preferred_attempt.output_path, request.output_path
                )
            },
        )?;

        Ok(ExportOutcome::Finished(request.output_path.clone()))
    })();

    cleanup_export_work_dir(&export_work_dir);
    export_result
}

#[cfg(not(feature = "ffmpeg"))]
fn attempt_export(
    request: &ClipExportRequest,
    output_path: &Path,
    video_bitrate_kbps: u32,
    audio_bitrate_kbps: u32,
    video_encoder: ExportVideoEncoder,
    progress_tx: &Sender<ClipExportUpdate>,
    cancel_flag: &Arc<AtomicBool>,
    attempt_index: usize,
    attempt_count: usize,
    single_pass_phase: ClipExportPhase,
) -> Result<Option<ExportAttemptResult>> {
    let first_pass_filter = build_filter_complex_for_request(request, false);
    let second_pass_filter = build_filter_complex_for_request(request, request.metadata.has_audio);
    let ffmpeg = ffmpeg_executable_path();
    let passlog_prefix = output_path.with_extension("passlog");
    let passlog_prefix_str = passlog_prefix.to_string_lossy().into_owned();

    let preset = select_adaptive_preset(video_bitrate_kbps, &request.metadata, video_encoder);

    let _ = progress_tx.send(ClipExportUpdate::Progress {
        phase: ClipExportPhase::Preparing,
        fraction: 0.0,
        message: "Preparing export".to_string(),
    });

    info!(
        "Exporting clipped video {:?} -> {:?} ({} kept ranges, target={} MB, video bitrate={} kbps, preset={}, encoder={})",
        request.input_path,
        output_path,
        request.keep_ranges.len(),
        request.target_size_mb,
        video_bitrate_kbps,
        preset,
        video_encoder.ffmpeg_name()
    );

    if video_encoder.supports_two_pass() {
        let first_pass_args = build_ffmpeg_args(
            request,
            &first_pass_filter,
            Some(&passlog_prefix_str),
            video_bitrate_kbps,
            audio_bitrate_kbps,
            true,
            &preset,
            output_path,
            video_encoder,
        );
        if run_ffmpeg_phase(
            &ffmpeg,
            &first_pass_args,
            request.output_duration_secs().max(0.1),
            FFmpegProgressContext {
                phase: ClipExportPhase::FirstPass,
                start_fraction: 0.0,
                span_fraction: 0.5,
                progress_tx: progress_tx.clone(),
                cancel_flag: cancel_flag.clone(),
                attempt_index,
                attempt_count,
                last_progress_time: std::time::Instant::now(),
            },
        )? {
            cleanup_passlog_files(&passlog_prefix);
            let _ = std::fs::remove_file(output_path);
            return Ok(None);
        }

        if cancel_flag.load(Ordering::Relaxed) {
            cleanup_passlog_files(&passlog_prefix);
            let _ = std::fs::remove_file(output_path);
            return Ok(None);
        }

        let second_pass_args = build_ffmpeg_args(
            request,
            &second_pass_filter,
            Some(&passlog_prefix_str),
            video_bitrate_kbps,
            audio_bitrate_kbps,
            false,
            &preset,
            output_path,
            video_encoder,
        );
        if run_ffmpeg_phase(
            &ffmpeg,
            &second_pass_args,
            request.output_duration_secs().max(0.1),
            FFmpegProgressContext {
                phase: ClipExportPhase::SecondPass,
                start_fraction: 0.5,
                span_fraction: 0.5,
                progress_tx: progress_tx.clone(),
                cancel_flag: cancel_flag.clone(),
                attempt_index,
                attempt_count,
                last_progress_time: std::time::Instant::now(),
            },
        )? {
            cleanup_passlog_files(&passlog_prefix);
            let _ = std::fs::remove_file(output_path);
            return Ok(None);
        }
    } else {
        let single_pass_args = build_ffmpeg_args(
            request,
            &second_pass_filter,
            None,
            video_bitrate_kbps,
            audio_bitrate_kbps,
            false,
            &preset,
            output_path,
            video_encoder,
        );
        if run_ffmpeg_phase(
            &ffmpeg,
            &single_pass_args,
            request.output_duration_secs().max(0.1),
            FFmpegProgressContext {
                phase: single_pass_phase,
                start_fraction: 0.0,
                span_fraction: 1.0,
                progress_tx: progress_tx.clone(),
                cancel_flag: cancel_flag.clone(),
                attempt_index,
                attempt_count,
                last_progress_time: std::time::Instant::now(),
            },
        )? {
            let _ = std::fs::remove_file(output_path);
            return Ok(None);
        }
    }

    let size_bytes = std::fs::metadata(output_path)
        .with_context(|| format!("Failed to get size of export output file {:?}", output_path))?
        .len();

    if video_encoder.supports_two_pass() {
        cleanup_passlog_files(&passlog_prefix);
    }

    Ok(Some(ExportAttemptResult {
        output_path: output_path.to_path_buf(),
        video_bitrate_kbps,
        size_bytes,
    }))
}

fn build_ffmpeg_args(
    request: &ClipExportRequest,
    filter_complex: &str,
    passlog_prefix: Option<&str>,
    video_bitrate_kbps: u32,
    audio_bitrate_kbps: u32,
    first_pass: bool,
    preset: &str,
    output_path: &Path,
    video_encoder: ExportVideoEncoder,
) -> Vec<String> {
    let mut args = vec![
        "-y".to_string(),
        "-hide_banner".to_string(),
        "-loglevel".to_string(),
        "error".to_string(),
        "-nostats".to_string(),
        "-progress".to_string(),
        "pipe:1".to_string(),
        "-i".to_string(),
        request.input_path.to_string_lossy().into_owned(),
    ];
    if let Some(w) = &request.webcam {
        args.push("-i".to_string());
        args.push(w.path.to_string_lossy().into_owned());
    }
    args.extend([
        "-filter_complex".to_string(),
        filter_complex.to_string(),
        "-map".to_string(),
        "[outv]".to_string(),
        "-c:v".to_string(),
        video_encoder.ffmpeg_name().to_string(),
        "-pix_fmt".to_string(),
        "yuv420p".to_string(),
        "-b:v".to_string(),
        format!("{video_bitrate_kbps}k"),
        "-maxrate".to_string(),
        format!("{video_bitrate_kbps}k"),
        "-bufsize".to_string(),
        format!("{}k", video_bitrate_kbps.saturating_mul(2)),
    ]);

    match video_encoder {
        ExportVideoEncoder::SoftwareHevc => {
            args.extend(["-preset".to_string(), preset.to_string()]);
            if let Some(passlog_prefix) = passlog_prefix {
                args.extend([
                    "-pass".to_string(),
                    if first_pass { "1" } else { "2" }.to_string(),
                    "-passlogfile".to_string(),
                    passlog_prefix.to_string(),
                ]);
            }
        }
        ExportVideoEncoder::HevcNvenc => {
            args.extend([
                "-preset".to_string(),
                preset.to_string(),
                "-rc".to_string(),
                "vbr".to_string(),
            ]);
        }
        ExportVideoEncoder::HevcAmf => {
            // Use VBR peak mode for export - recommended by AMD for recording use cases
            // vbr_latency is for low-latency streaming and doesn't respect bitrate targets well
            // vbr_peak allows encoder to vary bitrate up to maxrate for quality
            args.extend([
                "-quality".to_string(),
                preset.to_string(),
                "-rc".to_string(),
                "vbr_peak".to_string(),
                "-vbaq".to_string(),
                "true".to_string(),
                "-preencode".to_string(),
                "true".to_string(),
            ]);
        }
        ExportVideoEncoder::HevcQsv => {
            args.extend([
                "-preset".to_string(),
                preset.to_string(),
                "-look_ahead".to_string(),
                "1".to_string(),
            ]);
        }
    }

    if first_pass {
        args.extend([
            "-an".to_string(),
            "-f".to_string(),
            "null".to_string(),
            null_output_path().to_string(),
        ]);
        return args;
    }

    if request.metadata.has_audio {
        args.extend([
            "-map".to_string(),
            "[outa]".to_string(),
            "-c:a".to_string(),
            "aac".to_string(),
            "-b:a".to_string(),
            format!("{}k", audio_bitrate_kbps.max(48)),
        ]);
    }

    args.extend([
        "-movflags".to_string(),
        "+faststart".to_string(),
        output_path.to_string_lossy().into_owned(),
    ]);
    args
}

struct FFmpegProgressContext {
    phase: ClipExportPhase,
    start_fraction: f32,
    span_fraction: f32,
    progress_tx: Sender<ClipExportUpdate>,
    cancel_flag: Arc<AtomicBool>,
    attempt_index: usize,
    attempt_count: usize,
    last_progress_time: std::time::Instant,
}

fn run_ffmpeg_phase(
    ffmpeg: &Path,
    args: &[String],
    total_duration_secs: f64,
    mut progress_ctx: FFmpegProgressContext,
) -> Result<bool> {
    let mut command = Command::new(ffmpeg);
    command
        .args(args.iter().map(|arg| arg.as_str()))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    let mut child = command.spawn().with_context(|| {
        format!(
            "Failed to spawn ffmpeg phase {:?} via {:?}",
            progress_ctx.phase, ffmpeg
        )
    })?;

    let stderr = child
        .stderr
        .take()
        .context("ffmpeg phase stderr pipe was unavailable")?;
    let stderr_handle = thread::spawn(move || {
        let mut buffer = String::new();
        let _ = BufReader::new(stderr).read_to_string(&mut buffer);
        buffer
    });

    let stdout = child
        .stdout
        .take()
        .context("ffmpeg phase stdout pipe was unavailable")?;

    {
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            let line = line.context("Failed to read ffmpeg progress output")?;
            if progress_ctx.cancel_flag.load(Ordering::Relaxed) {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stderr_handle.join();
                return Ok(true);
            }

            if let Some(processed_secs) = parse_progress_seconds(&line) {
                let now = std::time::Instant::now();
                const MIN_PROGRESS_INTERVAL: std::time::Duration =
                    std::time::Duration::from_millis(100);

                if now.duration_since(progress_ctx.last_progress_time) >= MIN_PROGRESS_INTERVAL {
                    progress_ctx.last_progress_time = now;
                    let fraction = if total_duration_secs > 0.0 {
                        (processed_secs / total_duration_secs) as f32
                    } else {
                        0.0
                    };
                    let adjusted_fraction =
                        progress_ctx.start_fraction + (fraction * progress_ctx.span_fraction);
                    let _ = progress_ctx.progress_tx.send(ClipExportUpdate::Progress {
                        phase: progress_ctx.phase,
                        fraction: adjusted_fraction.clamp(0.0, 1.0),
                        message: format!(
                            "Attempt {}/{} - {}: {}",
                            progress_ctx.attempt_index + 1,
                            progress_ctx.attempt_count,
                            phase_label(progress_ctx.phase),
                            format_seconds_arg(processed_secs)
                        ),
                    });
                }
            }
        }
    }

    let status = child
        .wait()
        .context("Failed waiting for ffmpeg phase to finish")?;
    let stderr = stderr_handle.join().unwrap_or_default();

    if progress_ctx.cancel_flag.load(Ordering::SeqCst) {
        return Ok(true);
    }

    if !status.success() {
        bail!(
            "FFmpeg {} failed: {}",
            phase_label(progress_ctx.phase),
            stderr.trim()
        );
    }

    let _ = progress_ctx.progress_tx.send(ClipExportUpdate::Progress {
        phase: progress_ctx.phase,
        fraction: (progress_ctx.start_fraction + progress_ctx.span_fraction).clamp(0.0, 1.0),
        message: format!(
            "Attempt {}/{} - {} complete",
            progress_ctx.attempt_index + 1,
            progress_ctx.attempt_count,
            phase_label(progress_ctx.phase)
        ),
    });

    Ok(false)
}

fn build_filter_complex_for_request(request: &ClipExportRequest, has_audio: bool) -> String {
    if let Some(webcam) = &request.webcam {
        build_filter_with_webcam(request, webcam, has_audio)
    } else {
        build_filter_complex(&request.keep_ranges, has_audio, request.metadata.fps)
    }
}

fn build_filter_with_webcam(
    request: &ClipExportRequest,
    webcam: &WebcamExport,
    has_audio: bool,
) -> String {
    use super::webcam_layout::keyframes_for_output_timeline;

    let mw = request.metadata.width.max(1) as f64;
    let mh = request.metadata.height.max(1) as f64;

    let mut parts: Vec<String> = Vec::new();
    for (index, range) in request.keep_ranges.iter().enumerate() {
        // Add fps filter after trim+setpts to restore proper frame timing for hardware encoders
        parts.push(format!(
            "[0:v:0]trim=start={}:end={},setpts=PTS-STARTPTS,fps[v{index}]",
            format_seconds_arg(range.start_secs),
            format_seconds_arg(range.end_secs),
        ));
        if has_audio {
            parts.push(format!(
                "[0:a:0]atrim=start={}:end={},asetpts=PTS-STARTPTS[a{index}]",
                format_seconds_arg(range.start_secs),
                format_seconds_arg(range.end_secs),
            ));
        }
    }
    for (index, range) in request.keep_ranges.iter().enumerate() {
        // Add fps filter for webcam stream as well
        parts.push(format!(
            "[1:v:0]trim=start={}:end={},setpts=PTS-STARTPTS,fps[wv{index}]",
            format_seconds_arg(range.start_secs),
            format_seconds_arg(range.end_secs),
        ));
    }

    let mut c0 = String::new();
    for index in 0..request.keep_ranges.len() {
        c0.push_str(&format!("[v{index}]"));
        if has_audio {
            c0.push_str(&format!("[a{index}]"));
        }
    }
    let n = request.keep_ranges.len();
    if has_audio {
        c0.push_str(&format!("concat=n={n}:v=1:a=1[mv][outa]"));
    } else {
        c0.push_str(&format!("concat=n={n}:v=1:a=0[mv]"));
    }
    parts.push(c0);

    let mut c1 = String::new();
    for index in 0..n {
        c1.push_str(&format!("[wv{index}]"));
    }
    c1.push_str(&format!("concat=n={n}:v=1:a=0[wc]"));
    parts.push(c1);

    let mut kf = keyframes_for_output_timeline(&webcam.keyframes, &request.keep_ranges);
    if kf.is_empty() {
        kf = keyframes_for_output_timeline(
            &super::webcam_layout::default_webcam_keyframes(),
            &request.keep_ranges,
        );
    }
    let x_expr = piecewise_linear_expr_t(&kf.iter().map(|k| (k.t_secs, k.x)).collect::<Vec<_>>());
    let y_expr = piecewise_linear_expr_t(&kf.iter().map(|k| (k.t_secs, k.y)).collect::<Vec<_>>());
    let w_expr = piecewise_linear_expr_t(&kf.iter().map(|k| (k.t_secs, k.w)).collect::<Vec<_>>());
    let h_expr = piecewise_linear_expr_t(&kf.iter().map(|k| (k.t_secs, k.h)).collect::<Vec<_>>());

    let scale_w = format!("'{}*({})'", mw, w_expr);
    let scale_h = format!("'{}*({})'", mh, h_expr);
    let ox = format!("'W*({})'", x_expr);
    let oy = format!("'H*({})'", y_expr);

    parts.push(format!(
        "[wc]scale=w={}:h={}:eval=frame[wsc],[mv][wsc]overlay=x={}:y={}:format=auto[outv]",
        scale_w, scale_h, ox, oy
    ));

    parts.join(";")
}

/// Piecewise linear interpolation of `v` over `t` for ffmpeg `eval=frame` expressions.
fn piecewise_linear_expr_t(points: &[(f64, f64)]) -> String {
    let mut p = points.to_vec();
    if p.is_empty() {
        return "0".to_string();
    }
    p.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    if p.len() == 1 {
        return format!("{}", p[0].1);
    }
    let mut expr = format!("{}", p.last().unwrap().1);
    for i in (0..p.len() - 1).rev() {
        let (t0, v0) = p[i];
        let (t1, v1) = p[i + 1];
        let span = (t1 - t0).max(1e-6);
        let lerp = format!("{}+(t-{})/{}*({}-{})", v0, t0, span, v1, v0);
        expr = format!("if(between(t,{0},{1}),{2},{3})", t0, t1, lerp, expr);
    }
    let t0 = p[0].0;
    let v0 = p[0].1;
    format!("if(lt(t,{t0}),{v0},{expr})")
}

fn build_filter_complex(keep_ranges: &[TimeRange], has_audio: bool, fps: f64) -> String {
    let mut filters = Vec::new();

    for (index, range) in keep_ranges.iter().enumerate() {
        // Use fps filter with explicit rate to ensure proper frame timing after trim.
        // Without explicit fps, the filter may drop/duplicate frames at segment boundaries,
        // causing duration mismatches and A/V sync issues.
        // The fps filter also ensures keyframe-aware trimming by outputting at constant rate.
        filters.push(format!(
            "[0:v:0]trim=start={}:end={},setpts=PTS-STARTPTS,fps={}[v{index}]",
            format_seconds_arg(range.start_secs),
            format_seconds_arg(range.end_secs),
            fps,
        ));
        if has_audio {
            filters.push(format!(
                "[0:a:0]atrim=start={}:end={},asetpts=PTS-STARTPTS[a{index}]",
                format_seconds_arg(range.start_secs),
                format_seconds_arg(range.end_secs),
            ));
        }
    }

    let mut concat_inputs = String::new();
    for index in 0..keep_ranges.len() {
        concat_inputs.push_str(&format!("[v{index}]"));
        if has_audio {
            concat_inputs.push_str(&format!("[a{index}]"));
        }
    }

    concat_inputs.push_str(&format!(
        "concat=n={}:v=1:a={}",
        keep_ranges.len(),
        if has_audio { 1 } else { 0 }
    ));
    if has_audio {
        concat_inputs.push_str("[outv][outa]");
    } else {
        concat_inputs.push_str("[outv]");
    }

    filters.push(concat_inputs);
    filters.join(";")
}

fn parse_progress_seconds(line: &str) -> Option<f64> {
    let (_, value) = line.split_once('=')?;
    match line.split_once('=')?.0 {
        "out_time_ms" | "out_time_us" => {
            let micros = value.trim().parse::<f64>().ok()?;
            Some(micros / 1_000_000.0)
        }
        "out_time" => parse_hhmmss_time(value.trim()),
        _ => None,
    }
}

fn cleanup_passlog_files(prefix: &Path) {
    let Some(parent) = prefix.parent() else {
        return;
    };
    let Some(prefix_name) = prefix.file_name().and_then(|name| name.to_str()) else {
        return;
    };

    if let Ok(entries) = std::fs::read_dir(parent) {
        for entry in entries.flatten() {
            let file_name = entry.file_name();
            if file_name.to_string_lossy().starts_with(prefix_name) {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

fn cleanup_export_work_dir(work_dir: &Path) {
    let _ = std::fs::remove_dir_all(work_dir);
}

pub fn estimate_export_bitrates(
    target_size_mb: u32,
    output_duration_secs: f64,
    has_audio: bool,
    requested_audio_bitrate_kbps: u32,
    num_segments: usize,
    use_hardware_acceleration: bool,
) -> ExportBitrateEstimate {
    estimate_export_bitrates_with_fill_ratio(
        target_size_mb,
        output_duration_secs,
        has_audio,
        requested_audio_bitrate_kbps,
        num_segments,
        use_hardware_acceleration,
        MIN_VIDEO_BITRATE_KBPS,
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
) -> ExportBitrateEstimate {
    estimate_export_bitrates_with_fill_ratio_and_efficiency(
        target_size_mb,
        output_duration_secs,
        has_audio,
        requested_audio_bitrate_kbps,
        num_segments,
        encoder.min_video_bitrate_kbps(),
        encoder.bitrate_efficiency_factor(),
        encoder.initial_target_fill_ratio(),
    )
}

fn estimate_export_bitrates_with_fill_ratio(
    target_size_mb: u32,
    output_duration_secs: f64,
    has_audio: bool,
    requested_audio_bitrate_kbps: u32,
    num_segments: usize,
    use_hardware_acceleration: bool,
    min_video_bitrate_kbps: u32,
    target_fill_ratio: f64,
) -> ExportBitrateEstimate {
    let efficiency_factor = if use_hardware_acceleration {
        0.50
    } else {
        0.85
    };
    estimate_export_bitrates_with_fill_ratio_and_efficiency(
        target_size_mb,
        output_duration_secs,
        has_audio,
        requested_audio_bitrate_kbps,
        num_segments,
        min_video_bitrate_kbps,
        efficiency_factor,
        target_fill_ratio,
    )
}

fn estimate_export_bitrates_with_fill_ratio_and_efficiency(
    target_size_mb: u32,
    output_duration_secs: f64,
    has_audio: bool,
    requested_audio_bitrate_kbps: u32,
    num_segments: usize,
    min_video_bitrate_kbps: u32,
    efficiency_factor: f64,
    target_fill_ratio: f64,
) -> ExportBitrateEstimate {
    let duration_secs = output_duration_secs.max(0.1);
    let total_kbps = ((target_size_bytes(target_size_mb) as f64) * 8.0 / duration_secs / 1000.0)
        .max(f64::from(min_video_bitrate_kbps));
    let audio_kbps = select_audio_bitrate_kbps(has_audio, requested_audio_bitrate_kbps, total_kbps);
    let non_video_bytes = estimate_non_video_bytes(duration_secs, audio_kbps, num_segments);
    let filled_target_size_bytes = ((target_size_bytes(target_size_mb) as f64)
        * target_fill_ratio.clamp(0.1, 1.0))
    .round() as u64;
    let theoretical_video_kbps =
        ((((filled_target_size_bytes.saturating_sub(non_video_bytes)) as f64) * 8.0)
            / duration_secs
            / 1000.0)
            .max(f64::from(min_video_bitrate_kbps));

    let video_kbps = (theoretical_video_kbps * efficiency_factor)
        .max(f64::from(min_video_bitrate_kbps))
        .round() as u32;

    ExportBitrateEstimate {
        video_kbps,
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
    (32 * 1024) as u64 + stream_bytes
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

fn next_export_video_bitrate_kbps(
    encoder: ExportVideoEncoder,
    current_video_bitrate_kbps: u32,
    actual_size_bytes: u64,
    target_size_bytes: u64,
    estimated_non_video_bytes: u64,
    low_video_bitrate_kbps: u32,
    high_video_bitrate_kbps: Option<u32>,
) -> u32 {
    let current_video_bytes = actual_size_bytes
        .saturating_sub(estimated_non_video_bytes)
        .max(1);
    let ideal_target_video_bytes = desired_output_size_bytes(encoder, target_size_bytes)
        .saturating_sub(estimated_non_video_bytes)
        .max(1);
    if encoder.prefers_direct_retry() {
        return (f64::from(current_video_bitrate_kbps)
            * ((ideal_target_video_bytes as f64) / (current_video_bytes as f64)))
            .round()
            .clamp(100.0, MAX_VIDEO_BITRATE_KBPS as f64) as u32;
    }

    let target_video_bytes = target_size_bytes
        .saturating_sub(estimated_non_video_bytes)
        .max(1);

    // Calculate the exact ratio needed to hit target
    let byte_ratio = (target_video_bytes as f64) / (current_video_bytes as f64);
    let overshoot_ratio = (actual_size_bytes as f64) / (target_size_bytes as f64);

    // When over target, calculate the exact bitrate needed and aim for 95% of target
    // to ensure we land just under. When under, be conservative to avoid overshooting.
    let target_ratio = if actual_size_bytes > target_size_bytes {
        // Aim for 95% of target to ensure we're safely under without over-correcting
        byte_ratio * 0.95
    } else {
        // Slightly increase when under target to get closer without overshooting
        byte_ratio.min(1.05)
    };

    // Calculate target bitrate directly from ratio
    let calculated_bitrate = (f64::from(current_video_bitrate_kbps) * target_ratio)
        .round()
        .clamp(100.0, MAX_VIDEO_BITRATE_KBPS as f64) as u32;

    // Binary search bounds handling
    let mut next_video_bitrate_kbps = calculated_bitrate;

    if let Some(high_bound) = high_video_bitrate_kbps {
        let low_bound = low_video_bitrate_kbps;

        // If significantly over target (>10%), prioritize the calculated ratio
        // over binary search bounds to converge faster
        if overshoot_ratio > 1.10 {
            // Allow calculated bitrate even if below low_bound, but don't exceed high_bound
            next_video_bitrate_kbps = calculated_bitrate.min(high_bound).max(100);
        } else if high_bound <= low_bound.saturating_add(100) {
            // Bounds are close, use binary search midpoint for fine tuning
            next_video_bitrate_kbps =
                low_bound.saturating_add(high_bound.saturating_sub(low_bound) / 2);
        } else {
            // Clamp to binary search bounds
            next_video_bitrate_kbps = calculated_bitrate.clamp(low_bound, high_bound);

            // Force movement if stuck at same value
            if next_video_bitrate_kbps == current_video_bitrate_kbps {
                let step = ((high_bound.saturating_sub(low_bound)) / 4).max(50);
                if actual_size_bytes > target_size_bytes {
                    next_video_bitrate_kbps = current_video_bitrate_kbps
                        .saturating_sub(step)
                        .max(low_bound)
                        .max(100);
                } else {
                    next_video_bitrate_kbps = current_video_bitrate_kbps
                        .saturating_add(step)
                        .min(high_bound);
                }
            }
        }
    }

    next_video_bitrate_kbps.min(MAX_VIDEO_BITRATE_KBPS).max(100)
}

fn estimate_measured_non_video_bytes(
    actual_size_bytes: u64,
    current_video_bitrate_kbps: u32,
    output_duration_secs: f64,
) -> u64 {
    let expected_video_bytes = ((f64::from(current_video_bitrate_kbps) * 1000.0 / 8.0)
        * output_duration_secs.max(0.0))
    .round() as u64;
    actual_size_bytes.saturating_sub(expected_video_bytes)
}

fn blend_non_video_bytes_estimate(previous_estimate: u64, measured_estimate: u64) -> u64 {
    ((previous_estimate as f64 * 0.6) + (measured_estimate as f64 * 0.4)).round() as u64
}

fn calibration_duration_secs(output_duration_secs: f64) -> f64 {
    output_duration_secs
        .mul_add(0.20, 0.0)
        .clamp(5.0, 12.0)
        .min(output_duration_secs.max(0.1))
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

fn build_amf_calibration_request(
    request: &ClipExportRequest,
    calibration_duration_secs: f64,
    output_path: PathBuf,
) -> ClipExportRequest {
    let mut calibration_request = request.clone();
    calibration_request.output_path = output_path;
    let full_duration_secs = request.output_duration_secs().max(0.1);
    let sample_duration_secs = calibration_duration_secs.min(full_duration_secs);
    let window_count = if full_duration_secs >= sample_duration_secs * 3.0 {
        3
    } else if full_duration_secs >= sample_duration_secs * 2.0 {
        2
    } else {
        1
    };
    let window_duration_secs = (sample_duration_secs / window_count as f64)
        .max(0.5)
        .min(full_duration_secs);

    let mut keep_ranges = Vec::new();
    let window_starts: Vec<f64> = match window_count {
        1 => vec![0.0],
        2 => vec![0.0, (full_duration_secs - window_duration_secs).max(0.0)],
        _ => vec![
            0.0,
            ((full_duration_secs - window_duration_secs) / 2.0).max(0.0),
            (full_duration_secs - window_duration_secs).max(0.0),
        ],
    };

    for window_start_secs in window_starts {
        keep_ranges.extend(slice_keep_ranges_for_output_window(
            &request.keep_ranges,
            window_start_secs,
            window_duration_secs,
        ));
    }

    calibration_request.keep_ranges = keep_ranges;
    calibration_request
}

fn calibrate_amf_video_bitrate(
    current_video_bitrate_kbps: u32,
    sample_size_bytes: u64,
    sample_duration_secs: f64,
    sample_num_segments: usize,
    full_duration_secs: f64,
    full_num_segments: usize,
    audio_bitrate_kbps: u32,
    target_size_bytes: u64,
) -> u32 {
    let sample_duration_secs = sample_duration_secs.max(0.1);
    let full_duration_secs = full_duration_secs.max(sample_duration_secs);

    // Strip audio + container overhead before extrapolating. Overhead is NOT
    // proportional to duration: calibration uses multiple short segments (higher
    // per-second container overhead) while the full encode uses fewer segments.
    // Using total bytes causes a systematic undershoot, worst at low bitrates
    // where overhead is a larger fraction of the total.
    let sample_non_video =
        estimate_non_video_bytes(sample_duration_secs, audio_bitrate_kbps, sample_num_segments);
    let sample_video_bytes = sample_size_bytes.saturating_sub(sample_non_video).max(1);

    let full_non_video =
        estimate_non_video_bytes(full_duration_secs, audio_bitrate_kbps, full_num_segments);
    let desired_full_total =
        (target_size_bytes as f64 * AMF_CALIBRATION_FILL_RATIO).round() as u64;
    let desired_video_bytes = desired_full_total.saturating_sub(full_non_video).max(1);

    let extrapolated_full_video =
        (sample_video_bytes as f64) * (full_duration_secs / sample_duration_secs);
    if extrapolated_full_video <= 1.0 {
        return current_video_bitrate_kbps;
    }

    let calibrated = (f64::from(current_video_bitrate_kbps)
        * (desired_video_bytes as f64)
        / extrapolated_full_video)
        .round()
        .clamp(100.0, MAX_VIDEO_BITRATE_KBPS as f64) as u32;
    calibrated.max(MIN_AUTO_VIDEO_BITRATE_KBPS)
}

fn desired_output_size_bytes(encoder: ExportVideoEncoder, target_size_bytes: u64) -> u64 {
    ((target_size_bytes as f64) * encoder.initial_target_fill_ratio()).round() as u64
}

fn attempt_satisfies_target_window(
    encoder: ExportVideoEncoder,
    attempt_size_bytes: u64,
    target_size_bytes: u64,
) -> bool {
    if attempt_size_bytes > target_size_bytes {
        return false;
    }

    let (min_fill_ratio, max_fill_ratio) = encoder.acceptable_fill_range();
    let attempt_size_bytes = attempt_size_bytes as f64;
    let target_size_bytes = target_size_bytes as f64;
    attempt_size_bytes >= target_size_bytes * min_fill_ratio
        && attempt_size_bytes <= target_size_bytes * max_fill_ratio
}

fn should_replace_best_under_attempt(
    _encoder: ExportVideoEncoder,
    attempt_result: &ExportAttemptResult,
    best_attempt: Option<&ExportAttemptResult>,
    _target_size_bytes: u64,
) -> bool {
    let Some(best_attempt) = best_attempt else {
        return true;
    };

    attempt_result.size_bytes > best_attempt.size_bytes
        || (attempt_result.size_bytes == best_attempt.size_bytes
            && attempt_result.video_bitrate_kbps > best_attempt.video_bitrate_kbps)
}

fn select_preferred_attempt(
    best_under_target: Option<ExportAttemptResult>,
    best_over_target: Option<ExportAttemptResult>,
    target_size_bytes: u64,
    strict_under_target: bool,
) -> Option<ExportAttemptResult> {
    match (best_under_target, best_over_target) {
        (Some(under_target), Some(over_target)) => {
            if strict_under_target {
                return Some(under_target);
            }
            let under_delta = target_size_bytes.saturating_sub(under_target.size_bytes);
            let over_delta = over_target.size_bytes.saturating_sub(target_size_bytes);
            if over_delta < under_delta {
                Some(over_target)
            } else {
                Some(under_target)
            }
        }
        (Some(under_target), None) => Some(under_target),
        (None, Some(over_target)) => Some(over_target),
        (None, None) => None,
    }
}

fn select_export_video_encoder(request: &ClipExportRequest) -> Result<ExportVideoEncoder> {
    if !request.use_hardware_acceleration {
        return Ok(ExportVideoEncoder::SoftwareHevc);
    }

    // For SDK path, check available codecs via ffmpeg-next
    #[cfg(feature = "ffmpeg")]
    {
        let available_codecs = query_sdk_codecs();
        let detected_hevc_hardware: Vec<&str> = [
            ExportVideoEncoder::HevcNvenc,
            ExportVideoEncoder::HevcAmf,
            ExportVideoEncoder::HevcQsv,
        ]
        .into_iter()
        .filter(|encoder| available_codecs.contains(encoder.ffmpeg_name()))
        .map(|encoder| encoder.ffmpeg_name())
        .collect();

        info!(
            hardware_encoders = %detected_hevc_hardware.join(","),
            "Detected HEVC hardware encoders from SDK"
        );

        let requested_order: &[ExportVideoEncoder] = &[
            ExportVideoEncoder::HevcAmf,
            ExportVideoEncoder::HevcNvenc,
            ExportVideoEncoder::HevcQsv,
        ];

        for encoder in requested_order {
            if available_codecs.contains(encoder.ffmpeg_name()) {
                return Ok(*encoder);
            }
        }

        for encoder in [
            ExportVideoEncoder::HevcAmf,
            ExportVideoEncoder::HevcNvenc,
            ExportVideoEncoder::HevcQsv,
        ] {
            if available_codecs.contains(encoder.ffmpeg_name()) {
                return Ok(encoder);
            }
        }

        return Ok(ExportVideoEncoder::SoftwareHevc);
    }

    #[cfg(not(feature = "ffmpeg"))]
    {
        let ffmpeg = ffmpeg_executable_path();
        let available_encoders = query_ffmpeg_encoders(&ffmpeg)?;
        let detected_hevc_hardware: Vec<&str> = [
            ExportVideoEncoder::HevcNvenc,
            ExportVideoEncoder::HevcAmf,
            ExportVideoEncoder::HevcQsv,
        ]
        .into_iter()
        .filter(|encoder| available_encoders.contains(encoder.ffmpeg_name()))
        .map(|encoder| encoder.ffmpeg_name())
        .collect();

        info!(
            hardware_encoders = %detected_hevc_hardware.join(","),
            "Detected HEVC hardware encoders from FFmpeg CLI"
        );

        let requested_order: &[ExportVideoEncoder] = &[
            ExportVideoEncoder::HevcAmf,
            ExportVideoEncoder::HevcNvenc,
            ExportVideoEncoder::HevcQsv,
        ];

        for encoder in requested_order {
            if available_encoders.contains(encoder.ffmpeg_name()) {
                return Ok(*encoder);
            }
        }

        for encoder in [
            ExportVideoEncoder::HevcAmf,
            ExportVideoEncoder::HevcNvenc,
            ExportVideoEncoder::HevcQsv,
        ] {
            if available_encoders.contains(encoder.ffmpeg_name()) {
                return Ok(encoder);
            }
        }

        Ok(ExportVideoEncoder::SoftwareHevc)
    }
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

#[cfg(not(feature = "ffmpeg"))]
fn query_ffmpeg_encoders(ffmpeg: &Path) -> Result<String> {
    let output = command_output(
        Command::new(ffmpeg)
            .args(["-hide_banner", "-encoders"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null()),
    )
    .with_context(|| format!("Failed to query ffmpeg encoders via {:?}", ffmpeg))?;

    if !output.status.success() {
        bail!("ffmpeg encoder query failed via {:?}", ffmpeg);
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
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

fn select_adaptive_preset(
    video_bitrate_kbps: u32,
    metadata: &VideoFileMetadata,
    encoder: ExportVideoEncoder,
) -> &'static str {
    // Adaptive preset selection: use slower presets at lower bitrates
    // for better compression efficiency, and faster presets at high bitrates
    let pixels_per_frame = f64::from(metadata.width.max(1)) * f64::from(metadata.height.max(1));
    let bits_per_pixel_frame =
        (f64::from(video_bitrate_kbps) * 1000.0) / (pixels_per_frame * metadata.fps.max(1.0));
    let high_throughput_source = pixels_per_frame * metadata.fps.max(1.0) >= 2560.0 * 1440.0 * 60.0;

    match encoder {
        ExportVideoEncoder::SoftwareHevc => {
            if bits_per_pixel_frame < 0.05 {
                "veryslow"
            } else if bits_per_pixel_frame < 0.09 {
                if high_throughput_source {
                    "slower"
                } else {
                    "veryslow"
                }
            } else if bits_per_pixel_frame < 0.16 {
                "slower"
            } else if high_throughput_source {
                "slow"
            } else {
                "slower"
            }
        }
        ExportVideoEncoder::HevcNvenc => {
            if bits_per_pixel_frame < 0.1 {
                "p7"
            } else {
                "p6"
            }
        }
        ExportVideoEncoder::HevcAmf => "quality",
        ExportVideoEncoder::HevcQsv => {
            if high_throughput_source {
                "slow"
            } else {
                "slower"
            }
        }
    }
}

fn phase_label(phase: ClipExportPhase) -> &'static str {
    match phase {
        ClipExportPhase::Preparing => "Preparing export",
        ClipExportPhase::Calibration => "Calibration pass",
        ClipExportPhase::FirstPass => "Encoding pass 1",
        ClipExportPhase::SecondPass => "Encoding pass 2",
    }
}

#[cfg(all(feature = "ffmpeg-cli", not(feature = "ffmpeg")))]
fn ffprobe_executable_path() -> PathBuf {
    let ffmpeg = ffmpeg_executable_path();
    let sibling = ffmpeg.with_file_name(if cfg!(target_os = "windows") {
        "ffprobe.exe"
    } else {
        "ffprobe"
    });

    if sibling.exists() {
        sibling
    } else {
        PathBuf::from("ffprobe")
    }
}

#[cfg(not(feature = "ffmpeg"))]
fn command_output(command: &mut Command) -> Result<std::process::Output> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    Ok(command.output()?)
}

#[cfg(not(feature = "ffmpeg"))]
fn format_seconds_arg(seconds: f64) -> String {
    format!("{:.3}", seconds.max(0.0))
}

#[cfg(all(feature = "ffmpeg-cli", not(feature = "ffmpeg")))]
fn parse_rational_fps(value: &str) -> Option<f64> {
    if let Some((num, den)) = value.split_once('/') {
        let n = num.trim().parse::<f64>().ok()?;
        let d = den.trim().parse::<f64>().ok()?;
        if d > 0.0 {
            return Some(n / d);
        }
    }
    value.trim().parse::<f64>().ok()
}

fn parse_hhmmss_time(value: &str) -> Option<f64> {
    let mut total = 0.0;
    let parts: Vec<_> = value.split(':').collect();
    if parts.len() != 3 {
        return None;
    }
    for part in parts {
        total = total * 60.0 + part.trim().parse::<f64>().ok()?;
    }
    Some(total)
}

#[cfg(not(feature = "ffmpeg"))]
fn null_output_path() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        "NUL"
    }

    #[cfg(not(target_os = "windows"))]
    {
        "/dev/null"
    }
}

#[derive(Debug, Clone)]
struct ExportAttemptResult {
    output_path: PathBuf,
    video_bitrate_kbps: u32,
    size_bytes: u64,
}

#[cfg(feature = "ffmpeg")]
fn query_sdk_codecs() -> String {
    use ffmpeg_next::codec::Id;
    let mut result = String::new();
    
    // Check for HEVC encoders
    for id in [Id::HEVC, Id::H265] {
        if let Some(codec) = ffmpeg_next::encoder::find(id) {
            result.push_str(&format!("{}\n", codec.name()));
        }
    }
    
    // Check for hardware HEVC encoders by name
    for name in ["libx265", "hevc_nvenc", "hevc_amf", "hevc_qsv"] {
        if ffmpeg_next::encoder::find_by_name(name).is_some() {
            result.push_str(&format!("{}\n", name));
        }
    }
    
    result
}

#[cfg(feature = "ffmpeg")]
fn attempt_export(
    request: &ClipExportRequest,
    output_path: &Path,
    video_bitrate_kbps: u32,
    audio_bitrate_kbps: u32,
    video_encoder: ExportVideoEncoder,
    progress_tx: &Sender<ClipExportUpdate>,
    cancel_flag: &Arc<AtomicBool>,
    attempt_index: usize,
    attempt_count: usize,
    single_pass_phase: ClipExportPhase,
) -> Result<Option<ExportAttemptResult>> {
    // SDK path delegates to CLI subprocess for trimming operations.
    // The ffmpeg-next filter graph API is complex and the CLI path already works correctly
    // with proper trim/setpts/concat for continuous timestamps.
    use std::time::Instant;
    
    let total_duration_secs = request.output_duration_secs().max(0.1);
    let preset = select_adaptive_preset(video_bitrate_kbps, &request.metadata, video_encoder);

    let _ = progress_tx.send(ClipExportUpdate::Progress {
        phase: ClipExportPhase::Preparing,
        fraction: 0.0,
        message: "Preparing export".to_string(),
    });

    info!(
        "Exporting clipped video (SDK->CLI) {:?} -> {:?} ({} kept ranges, target={} MB, video bitrate={} kbps, preset={}, encoder={})",
        request.input_path,
        output_path,
        request.keep_ranges.len(),
        request.target_size_mb,
        video_bitrate_kbps,
        preset,
        video_encoder.ffmpeg_name()
    );

    // Build filter complex for trimming
    let filter_complex = build_filter_complex_for_request(request, request.metadata.has_audio);
    let ffmpeg = ffmpeg_executable_path();
    let passlog_prefix = output_path.with_extension("passlog");
    let passlog_prefix_str = passlog_prefix.to_string_lossy().into_owned();

    if video_encoder.supports_two_pass() {
        // Two-pass encoding for software encoder
        let first_pass_args = build_ffmpeg_args(
            request,
            &filter_complex,
            Some(&passlog_prefix_str),
            video_bitrate_kbps,
            audio_bitrate_kbps,
            true,
            &preset,
            output_path,
            video_encoder,
        );
        if run_ffmpeg_phase(
            &ffmpeg,
            &first_pass_args,
            total_duration_secs,
            FFmpegProgressContext {
                phase: ClipExportPhase::FirstPass,
                start_fraction: 0.0,
                span_fraction: 0.5,
                progress_tx: progress_tx.clone(),
                cancel_flag: cancel_flag.clone(),
                attempt_index,
                attempt_count,
                last_progress_time: Instant::now(),
            },
        )? {
            cleanup_passlog_files(&passlog_prefix);
            let _ = std::fs::remove_file(output_path);
            return Ok(None);
        }

        if cancel_flag.load(Ordering::Relaxed) {
            cleanup_passlog_files(&passlog_prefix);
            let _ = std::fs::remove_file(output_path);
            return Ok(None);
        }

        let second_pass_args = build_ffmpeg_args(
            request,
            &filter_complex,
            Some(&passlog_prefix_str),
            video_bitrate_kbps,
            audio_bitrate_kbps,
            false,
            &preset,
            output_path,
            video_encoder,
        );
        if run_ffmpeg_phase(
            &ffmpeg,
            &second_pass_args,
            total_duration_secs,
            FFmpegProgressContext {
                phase: ClipExportPhase::SecondPass,
                start_fraction: 0.5,
                span_fraction: 0.5,
                progress_tx: progress_tx.clone(),
                cancel_flag: cancel_flag.clone(),
                attempt_index,
                attempt_count,
                last_progress_time: Instant::now(),
            },
        )? {
            cleanup_passlog_files(&passlog_prefix);
            let _ = std::fs::remove_file(output_path);
            return Ok(None);
        }

        cleanup_passlog_files(&passlog_prefix);
    } else {
        // Single-pass encoding for hardware encoders
        let args = build_ffmpeg_args(
            request,
            &filter_complex,
            None,
            video_bitrate_kbps,
            audio_bitrate_kbps,
            false,
            &preset,
            output_path,
            video_encoder,
        );
        if run_ffmpeg_phase(
            &ffmpeg,
            &args,
            total_duration_secs,
            FFmpegProgressContext {
                phase: single_pass_phase,
                start_fraction: 0.0,
                span_fraction: 1.0,
                progress_tx: progress_tx.clone(),
                cancel_flag: cancel_flag.clone(),
                attempt_index,
                attempt_count,
                last_progress_time: Instant::now(),
            },
        )? {
            let _ = std::fs::remove_file(output_path);
            return Ok(None);
        }
    }

    let size_bytes = std::fs::metadata(output_path)
        .with_context(|| format!("Failed to get size of export output file {:?}", output_path))?
        .len();

    Ok(Some(ExportAttemptResult {
        output_path: output_path.to_path_buf(),
        video_bitrate_kbps,
        size_bytes,
    }))
}

#[cfg(feature = "ffmpeg")]
fn format_seconds_arg(seconds: f64) -> String {
    format!("{:.3}", seconds.max(0.0))
}

#[cfg(feature = "ffmpeg")]
fn null_output_path() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        "NUL"
    }
    #[cfg(not(target_os = "windows"))]
    {
        "/dev/null"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_progress_output_variants() {
        assert_eq!(parse_progress_seconds("out_time_ms=1500000"), Some(1.5));
        assert_eq!(
            parse_progress_seconds("out_time=00:00:02.500000"),
            Some(2.5)
        );
        assert_eq!(parse_progress_seconds("progress=continue"), None);
    }

    #[test]
    fn estimate_export_bitrates_scales_audio_down_for_small_budgets() {
        // Software encoder estimate
        let estimate = estimate_export_bitrates(1, 20.0, true, 128, 2, false);

        assert_eq!(estimate.audio_kbps, 64);
        assert!(estimate.video_kbps >= MIN_VIDEO_BITRATE_KBPS);
        assert!(estimate.total_kbps >= estimate.video_kbps);

        // Hardware encoder should have lower initial video bitrate
        let hw_estimate = estimate_export_bitrates(1, 20.0, true, 128, 2, true);
        assert!(hw_estimate.video_kbps < estimate.video_kbps);
    }

    #[test]
    fn amf_initial_estimate_targets_the_90_to_95_percent_band() {
        let estimate =
            estimate_export_bitrates_for_encoder(3, 84.0, false, 0, 1, ExportVideoEncoder::HevcAmf);
        let generic_hw_estimate = estimate_export_bitrates(3, 84.0, false, 0, 1, true);

        assert!(estimate.video_kbps > generic_hw_estimate.video_kbps);
        assert!((240..=290).contains(&estimate.video_kbps));
        assert_eq!(
            desired_output_size_bytes(ExportVideoEncoder::HevcAmf, target_size_bytes(3)),
            ((target_size_bytes(3) as f64) * AMF_TARGET_FILL_RATIO_IDEAL).round() as u64
        );
    }

    #[test]
    fn next_export_video_bitrate_tracks_video_budget_instead_of_total_size() {
        let next_video_bitrate_kbps = next_export_video_bitrate_kbps(
            ExportVideoEncoder::SoftwareHevc,
            4000,
            4_500_000,
            4_000_000,
            1_500_000,
            MIN_VIDEO_BITRATE_KBPS,
            None,
        );

        assert!(next_video_bitrate_kbps < 4000);
        assert!(next_video_bitrate_kbps > 3000);
    }

    #[test]
    fn amf_retry_uses_measured_output_to_jump_to_the_target_band() {
        let next_video_bitrate_kbps = next_export_video_bitrate_kbps(
            ExportVideoEncoder::HevcAmf,
            300,
            3_227_539,
            3_145_728,
            0,
            MIN_VIDEO_BITRATE_KBPS,
            None,
        );

        assert!((265..=275).contains(&next_video_bitrate_kbps));
    }

    #[test]
    fn calibration_request_slices_across_the_clip() {
        let request = ClipExportRequest {
            input_path: PathBuf::from("input.mp4"),
            output_path: PathBuf::from("output.mp4"),
            keep_ranges: vec![
                TimeRange {
                    start_secs: 0.0,
                    end_secs: 5.0,
                },
                TimeRange {
                    start_secs: 10.0,
                    end_secs: 20.0,
                },
            ],
            target_size_mb: 3,
            audio_bitrate_kbps: 128,
            use_hardware_acceleration: true,
            preferred_encoder: EncoderType::Auto,
            metadata: VideoFileMetadata {
                duration_secs: 20.0,
                width: 1920,
                height: 1080,
                has_audio: true,
                fps: 60.0,
            },
            webcam: None,
        };

        let calibration =
            build_amf_calibration_request(&request, 6.0, PathBuf::from("calibration.mp4"));

        assert_eq!(calibration.output_duration_secs(), 6.0);
        assert_eq!(calibration.keep_ranges.len(), 2);
        assert_eq!(
            calibration.keep_ranges[0],
            TimeRange {
                start_secs: 0.0,
                end_secs: 3.0,
            }
        );
        assert_eq!(
            calibration.keep_ranges[1],
            TimeRange {
                start_secs: 17.0,
                end_secs: 20.0,
            }
        );
    }

    #[test]
    fn calibration_bitrate_scales_from_the_sample_size() {
        // 3 calibration segments, 1 full segment, 48 kbps audio.
        // Video-only extrapolation with AMF_CALIBRATION_FILL_RATIO=0.990 pushes the
        // calibrated bitrate higher than the old total-bytes formula to compensate
        // for AMF's non-linear overshoot and per-segment container overhead bias.
        let calibrated =
            calibrate_amf_video_bitrate(300, 600_000, 4.0, 3, 20.0, 1, 48, target_size_bytes(3));
        assert!((315..=345).contains(&calibrated));
    }

    #[test]
    fn amf_prefers_under_target_attempt_closest_to_ideal_fill() {
        let target_size_bytes = target_size_bytes(3);
        let farther_attempt = ExportAttemptResult {
            output_path: PathBuf::from("attempt-1.mp4"),
            video_bitrate_kbps: 300,
            size_bytes: ((target_size_bytes as f64) * 0.97).round() as u64,
        };
        let closer_attempt = ExportAttemptResult {
            output_path: PathBuf::from("attempt-2.mp4"),
            video_bitrate_kbps: 270,
            size_bytes: ((target_size_bytes as f64) * 0.924).round() as u64,
        };

        assert!(!should_replace_best_under_attempt(
            ExportVideoEncoder::HevcAmf,
            &closer_attempt,
            Some(&farther_attempt),
            target_size_bytes,
        ));
        assert!(attempt_satisfies_target_window(
            ExportVideoEncoder::HevcAmf,
            closer_attempt.size_bytes,
            target_size_bytes,
        ));
        assert!(attempt_satisfies_target_window(
            ExportVideoEncoder::HevcAmf,
            farther_attempt.size_bytes,
            target_size_bytes,
        ));
    }

    #[cfg(all(feature = "ffmpeg-cli", not(feature = "ffmpeg")))]
    #[test]
    fn exports_trimmed_snippets_with_ffmpeg() {
        let ffmpeg = ffmpeg_executable_path();
        if !ffmpeg.exists() {
            return;
        }

        let temp_dir = std::env::temp_dir().join(format!(
            "liteclip-export-test-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_millis()
        ));
        std::fs::create_dir_all(&temp_dir).unwrap();

        let input_path = temp_dir.join("input.mp4");
        let output_path = temp_dir.join("output.mp4");

        let sample_status = command_output(Command::new(&ffmpeg).args([
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc=size=320x240:rate=30",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=1000:sample_rate=48000",
            "-t",
            "2",
            "-c:v",
            "libx265",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
            &input_path.to_string_lossy(),
        ]))
        .unwrap();
        assert!(sample_status.status.success());

        let metadata = probe_video_file(&input_path).unwrap();
        let (progress_tx, _progress_rx) = std::sync::mpsc::channel();
        let cancel_flag = Arc::new(AtomicBool::new(false));

        let outcome = run_clip_export(
            &ClipExportRequest {
                input_path: input_path.clone(),
                output_path: output_path.clone(),
                keep_ranges: vec![
                    TimeRange {
                        start_secs: 0.0,
                        end_secs: 0.5,
                    },
                    TimeRange {
                        start_secs: 1.0,
                        end_secs: 1.5,
                    },
                ],
                target_size_mb: 2,
                audio_bitrate_kbps: 96,
                use_hardware_acceleration: false,
                preferred_encoder: EncoderType::Auto,
                metadata,
                webcam: None,
            },
            &progress_tx,
            &cancel_flag,
        )
        .unwrap();

        match outcome {
            ExportOutcome::Finished(path) => assert!(path.exists()),
            ExportOutcome::Cancelled => panic!("export unexpectedly cancelled"),
        }

        let exported = probe_video_file(&output_path).unwrap();
        assert!(exported.duration_secs > 0.8);
        assert!(exported.duration_secs < 1.3);

        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
