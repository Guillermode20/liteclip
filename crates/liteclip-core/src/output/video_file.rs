use crate::config::EncoderType;
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
    /// Output resolution. None means auto (based on target size) or original if stream_copy.
    pub output_width: Option<u32>,
    pub output_height: Option<u32>,
    /// Output frame rate. None means use original.
    pub output_fps: Option<f64>,
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

const MIN_VIDEO_BITRATE_KBPS: u32 = 300;
const MIN_AUTO_VIDEO_BITRATE_KBPS: u32 = 100;
const MAX_EXPORT_ATTEMPTS: usize = 6;
const AMF_MAX_EXPORT_ATTEMPTS: usize = 4;
const AMF_MIN_CALIBRATION_DURATION_SECS: f64 = 30.0; // Skip calibration for shorter clips
const MIN_REASONABLE_FPS: f64 = 1.0;
const MAX_REASONABLE_FPS: f64 = 240.0;
const FALLBACK_EXPORT_FPS: f64 = 60.0;
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

#[derive(Debug, Clone, Copy)]
struct ExportPrototypeFlags {
    software_two_pass: bool,
    export_hw_decode: bool,
    filter_graph_concat: bool,
}

impl ExportPrototypeFlags {
    fn from_env() -> Self {
        Self {
            software_two_pass: env_flag("LITECLIP_EXPORT_PROTO_TWO_PASS_SW"),
            export_hw_decode: env_flag("LITECLIP_EXPORT_PROTO_HW_DECODE"),
            filter_graph_concat: env_flag("LITECLIP_EXPORT_PROTO_FILTER_GRAPH"),
        }
    }
}

fn env_flag(key: &str) -> bool {
    match std::env::var(key) {
        Ok(value) => matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        Err(_) => false,
    }
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
        // SDK export path does not implement two-pass encoding
        // Size accuracy is handled via iterative bitrate adjustment instead
        false
    }

    fn initial_target_fill_ratio(self) -> f64 {
        match self {
            ExportVideoEncoder::HevcAmf => AMF_TARGET_FILL_RATIO_IDEAL,
            _ => 1.0,
        }
    }

    fn acceptable_fill_range(self) -> (f64, f64) {
        match self {
            // AMF must stay between 90% and 100% of target (no over-target allowed)
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

    fn initial_high_bitrate_kbps(self, initial_video_bitrate_kbps: u32) -> Option<u32> {
        let multiplier = match self {
            ExportVideoEncoder::SoftwareHevc => Some(1.55),
            ExportVideoEncoder::HevcAmf => Some(1.75),
            ExportVideoEncoder::HevcNvenc | ExportVideoEncoder::HevcQsv => None,
        }?;

        let high = ((initial_video_bitrate_kbps as f64) * multiplier).round() as u32;
        Some(
            high.min(MAX_VIDEO_BITRATE_KBPS)
                .max(initial_video_bitrate_kbps.saturating_add(100)),
        )
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
        return crate::output::sdk_ffmpeg_output::probe_video_file(video_path);
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
        return crate::output::sdk_ffmpeg_output::extract_preview_frame(
            video_path,
            timestamp_secs,
            max_width,
        );
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
    let export_started_at = Instant::now();
    let mut attempt_duration_total_secs = 0.0f64;
    let mut attempt_count_executed = 0usize;
    let prototype_flags = ExportPrototypeFlags::from_env();
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

    // Stream copy mode: fast export without re-encoding
    if request.stream_copy {
        #[cfg(feature = "ffmpeg")]
        {
            let stream_copy_result =
                super::sdk_export::run_stream_copy_export_sdk(request, progress_tx, cancel_flag);
            info!(
                elapsed_secs = export_started_at.elapsed().as_secs_f64(),
                output = ?request.output_path,
                "Stream copy export path completed"
            );
            return stream_copy_result;
        }
        #[cfg(not(feature = "ffmpeg"))]
        {
            anyhow::bail!("ffmpeg feature is required for stream copy export");
        }
    }

    let output_duration_secs = request.output_duration_secs().max(0.1);
    info!(
        input = ?request.input_path,
        output = ?request.output_path,
        output_duration_secs,
        keep_ranges = request.keep_ranges.len(),
        target_size_mb = request.target_size_mb,
        output_width = ?request.output_width,
        output_height = ?request.output_height,
        output_fps = ?request.output_fps,
        software_two_pass_proto = prototype_flags.software_two_pass,
        export_hw_decode_proto = prototype_flags.export_hw_decode,
        filter_graph_concat_proto = prototype_flags.filter_graph_concat,
        "Starting clip export encode pipeline"
    );
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
    let _work_dir_guard = WorkDirGuard::new(export_work_dir.clone());

    if selected_encoder == ExportVideoEncoder::HevcAmf
        && output_duration_secs >= AMF_MIN_CALIBRATION_DURATION_SECS
    {
        let sample_duration_secs = calibration_duration_secs(output_duration_secs);
        let calibration_path = export_work_dir.join("amf-calibration.mp4");
        // Calibration path only supported with ffmpeg SDK
        #[cfg(not(feature = "ffmpeg"))]
        if selected_encoder == ExportVideoEncoder::HevcAmf {
            anyhow::bail!("AMF calibration requires ffmpeg feature");
        }
        let calibration_request =
            build_amf_calibration_request(request, sample_duration_secs, calibration_path.clone());
        if prototype_flags.filter_graph_concat {
            info!(
                "Filter-graph export prototype flag is enabled. Calibration/export currently uses seek-based range processing; filter-graph path is planned but not yet wired."
            );
        }
        if prototype_flags.export_hw_decode {
            info!(
                "Hardware decode export prototype flag is enabled. Export decode currently remains software-decoded in attempt_export; D3D11VA export decode path is planned."
            );
        }
        let calibration_bitrate_kbps = bitrate_estimate
            .video_kbps
            .max(selected_encoder.min_video_bitrate_kbps());

        let mut calibration_result = None;
        match super::sdk_export::attempt_export(
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
        let mut high_video_bitrate_kbps =
            selected_encoder.initial_high_bitrate_kbps(current_video_bitrate_kbps);
        if selected_encoder == ExportVideoEncoder::SoftwareHevc && prototype_flags.software_two_pass
        {
            info!(
                "Software two-pass prototype flag is enabled. Current export path remains iterative; two-pass implementation is not yet wired."
            );
        }
        let mut best_under_target: Option<ExportAttemptResult> = None;
        let mut best_over_target: Option<ExportAttemptResult> = None;
        let mut amf_attempts = 0usize;

        for attempt_index in 0..MAX_EXPORT_ATTEMPTS {
            if cancel_flag.load(Ordering::Relaxed) {
                return Ok(ExportOutcome::Cancelled);
            }

            let output_path = export_work_dir.join(format!("attempt-{}.mp4", attempt_index + 1));
            let attempt_started_at = Instant::now();
            let attempt_result = match super::sdk_export::attempt_export(
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
            let attempt_elapsed_secs = attempt_started_at.elapsed().as_secs_f64();
            attempt_duration_total_secs += attempt_elapsed_secs;
            attempt_count_executed = attempt_count_executed.saturating_add(1);

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
            info!(
                attempt_index = attempt_index + 1,
                attempt_elapsed_secs,
                fill_ratio = (attempt_result.size_bytes as f64) / (target_size_bytes as f64),
                encoder = selected_encoder.ffmpeg_name(),
                "Export attempt instrumentation"
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
            select_preferred_attempt(best_under_target, best_over_target, target_size_bytes, selected_encoder)
                .with_context(|| {
                    format!(
                        "Unable to export within target size limit (max {} MB). Try increasing target size or reducing kept duration.",
                        request.target_size_mb
                    )
                })?;

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

    info!(
        elapsed_secs = export_started_at.elapsed().as_secs_f64(),
        attempt_duration_total_secs,
        attempt_count_executed,
        final_encoder = selected_encoder.ffmpeg_name(),
        "Clip export encode pipeline completed"
    );
    export_result
}

#[allow(dead_code)]
fn build_filter_complex_for_request(request: &ClipExportRequest, has_audio: bool) -> String {
    let fps = normalize_output_fps(
        request.output_fps.unwrap_or(request.metadata.fps),
        request.metadata.fps,
    );
    let (out_w, out_h) = if let (Some(w), Some(h)) = (request.output_width, request.output_height) {
        (w, h)
    } else {
        (request.metadata.width, request.metadata.height)
    };
    build_filter_complex(&request.keep_ranges, has_audio, fps, out_w, out_h)
}

#[allow(dead_code)]
fn build_filter_complex(
    keep_ranges: &[TimeRange],
    has_audio: bool,
    fps: f64,
    out_width: u32,
    out_height: u32,
) -> String {
    let mut filters = Vec::new();

    for (index, range) in keep_ranges.iter().enumerate() {
        // Use fps filter with explicit rate and optional scale
        let scale_filter = if out_width != 0 && out_height != 0 {
            format!(
                ",scale={}:{}:force_original_aspect_ratio=decrease:force_divisible_by=2",
                out_width, out_height
            )
        } else {
            String::new()
        };
        filters.push(format!(
            "[0:v:0]trim=start={}:end={},setpts=PTS-STARTPTS,fps={}{}[v{index}]",
            format_seconds_arg(range.start_secs),
            format_seconds_arg(range.end_secs),
            fps,
            scale_filter,
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

#[allow(dead_code)]
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
        let mut ratio = (ideal_target_video_bytes as f64) / (current_video_bytes as f64);
        if actual_size_bytes > target_size_bytes {
            ratio *= 0.97;
        }
        return (f64::from(current_video_bitrate_kbps) * ratio)
            .round()
            .clamp(100.0, MAX_VIDEO_BITRATE_KBPS as f64) as u32;
    }

    let target_video_bytes = target_size_bytes
        .saturating_sub(estimated_non_video_bytes)
        .max(1);

    // Calculate the exact ratio needed to hit target
    let byte_ratio = (target_video_bytes as f64) / (current_video_bytes as f64);
    let overshoot_ratio = (actual_size_bytes as f64) / (target_size_bytes as f64);

    // When over target, apply a stronger correction for larger overshoots to reduce
    // full-attempt count. When under, stay conservative to avoid bouncing over target.
    let target_ratio = if actual_size_bytes > target_size_bytes {
        let safety = if overshoot_ratio > 1.20 {
            0.88
        } else if overshoot_ratio > 1.10 {
            0.92
        } else {
            0.95
        };
        byte_ratio * safety
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
    let sample_non_video = estimate_non_video_bytes(
        sample_duration_secs,
        audio_bitrate_kbps,
        sample_num_segments,
    );
    let sample_video_bytes = sample_size_bytes.saturating_sub(sample_non_video).max(1);

    let full_non_video =
        estimate_non_video_bytes(full_duration_secs, audio_bitrate_kbps, full_num_segments);
    let desired_full_total = (target_size_bytes as f64 * AMF_CALIBRATION_FILL_RATIO).round() as u64;
    let desired_video_bytes = desired_full_total.saturating_sub(full_non_video).max(1);

    let extrapolated_full_video =
        (sample_video_bytes as f64) * (full_duration_secs / sample_duration_secs);
    if extrapolated_full_video <= 1.0 {
        return current_video_bitrate_kbps;
    }

    // Calculate the raw bitrate ratio needed to hit target
    let raw_bitrate_ratio = (desired_video_bytes as f64) / extrapolated_full_video;

    // AMF vbr_peak has non-linear overshoot: higher bitrates overshoot by larger percentages.
    // Apply a small compensation factor when extrapolating upward to account for this.
    // Observed: extrapolating from 793→972 kbps (ratio 1.23) produced 2% overshoot.
    // The overshoot grows roughly proportionally to the extrapolation ratio above 1.0.
    const UPWARD_EXTRAPOLATION_COMPENSATION: f64 = 0.015; // 1.5% per unit ratio above 1.0
    const MAX_UPWARD_EXTRAPOLATION_RATIO: f64 = 1.5;
    const EXTRAPOLATION_SAFETY_MARGIN: f64 = 0.85; // 15% safety margin when capping

    let (capped_ratio, was_capped) = if raw_bitrate_ratio > MAX_UPWARD_EXTRAPOLATION_RATIO {
        // Extrapolation ratio exceeds cap - apply both cap and safety margin
        (
            MAX_UPWARD_EXTRAPOLATION_RATIO * EXTRAPOLATION_SAFETY_MARGIN,
            true,
        )
    } else if raw_bitrate_ratio > 1.0 {
        // Upward extrapolation within normal range - apply proportional compensation
        // for non-linear VBR overshoot. Higher ratios need more compensation.
        let overshoot_compensation =
            1.0 - UPWARD_EXTRAPOLATION_COMPENSATION * (raw_bitrate_ratio - 1.0);
        (raw_bitrate_ratio * overshoot_compensation, false)
    } else {
        // Downward extrapolation - use as-is (undershoot is acceptable)
        (raw_bitrate_ratio, false)
    };

    let calibrated = (f64::from(current_video_bitrate_kbps) * capped_ratio)
        .round()
        .clamp(100.0, MAX_VIDEO_BITRATE_KBPS as f64) as u32;

    // If we capped the extrapolation, log a warning about potential quality limitation
    if was_capped {
        tracing::warn!(
            calibration_bitrate = current_video_bitrate_kbps,
            target_ratio = raw_bitrate_ratio,
            capped_ratio,
            "AMF calibration: capped upward extrapolation ratio to prevent overshoot. \
             Consider using higher calibration bitrate or software encoder for this target."
        );
    }

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
    encoder: ExportVideoEncoder,
) -> Option<ExportAttemptResult> {
    let (_min_fill, max_fill) = encoder.acceptable_fill_range();

    match (best_under_target, best_over_target) {
        (Some(under_target), Some(over_target)) => {
            // Check if over_target is within acceptable range for this encoder
            let over_fill_ratio = over_target.size_bytes as f64 / target_size_bytes as f64;
            if over_fill_ratio <= max_fill {
                // Both are acceptable, pick the one closer to target
                let under_delta = target_size_bytes.saturating_sub(under_target.size_bytes);
                let over_delta = over_target.size_bytes.saturating_sub(target_size_bytes);
                if over_delta < under_delta {
                    Some(over_target)
                } else {
                    Some(under_target)
                }
            } else {
                // Over-target is outside acceptable range, use under-target
                Some(under_target)
            }
        }
        (Some(under_target), None) => Some(under_target),
        (None, Some(over_target)) => {
            // Check if over_target is within acceptable range for this encoder
            let over_fill_ratio = over_target.size_bytes as f64 / target_size_bytes as f64;
            if over_fill_ratio <= max_fill {
                Some(over_target)
            } else {
                None
            }
        }
        (None, None) => None,
    }
}

/// Query available encoders via ffmpeg-next SDK
#[cfg(feature = "ffmpeg")]
fn query_sdk_codecs() -> Vec<&'static str> {
    use ffmpeg_next as ffmpeg;

    let mut available = Vec::new();

    // Check for hardware encoders by trying to find them
    if ffmpeg::encoder::find_by_name("hevc_nvenc").is_some() {
        available.push("hevc_nvenc");
    }
    if ffmpeg::encoder::find_by_name("hevc_amf").is_some() {
        available.push("hevc_amf");
    }
    if ffmpeg::encoder::find_by_name("hevc_qsv").is_some() {
        available.push("hevc_qsv");
    }
    // libx265 is always available in software
    available.push("libx265");

    available
}

fn ordered_hardware_encoder_preferences(preferred: EncoderType) -> Vec<ExportVideoEncoder> {
    let mut order = Vec::with_capacity(3);
    match preferred {
        EncoderType::Nvenc => order.push(ExportVideoEncoder::HevcNvenc),
        EncoderType::Amf => order.push(ExportVideoEncoder::HevcAmf),
        EncoderType::Qsv => order.push(ExportVideoEncoder::HevcQsv),
        EncoderType::Auto => {}
    }

    for encoder in [
        ExportVideoEncoder::HevcAmf,
        ExportVideoEncoder::HevcNvenc,
        ExportVideoEncoder::HevcQsv,
    ] {
        if !order.contains(&encoder) {
            order.push(encoder);
        }
    }
    order
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
        .filter(|encoder| available_codecs.contains(&encoder.ffmpeg_name()))
        .map(|encoder| encoder.ffmpeg_name())
        .collect();

        info!(
            hardware_encoders = %detected_hevc_hardware.join(","),
            "Detected HEVC hardware encoders from SDK"
        );

        let requested_order = ordered_hardware_encoder_preferences(request.preferred_encoder);
        info!(
            preferred_encoder = ?request.preferred_encoder,
            requested_order = %requested_order
                .iter()
                .map(|encoder| encoder.ffmpeg_name())
                .collect::<Vec<_>>()
                .join(","),
            "Resolved export encoder preference order"
        );

        for encoder in &requested_order {
            if available_codecs.contains(&encoder.ffmpeg_name()) {
                return Ok(*encoder);
            }
        }

        Ok(ExportVideoEncoder::SoftwareHevc)
    }

    #[cfg(not(feature = "ffmpeg"))]
    {
        // Without ffmpeg SDK, hardware acceleration is not available for export
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

fn format_seconds_arg(seconds: f64) -> String {
    format!("{:.3}", seconds.max(0.0))
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
        // 1 MB / 20s = ~419 kbps total, so audio gets scaled down to 48 kbps
        let estimate = estimate_export_bitrates(1, 20.0, true, 128, 2, false);

        assert_eq!(estimate.audio_kbps, 48); // Scaled down due to small budget
        assert!(estimate.video_kbps >= MIN_VIDEO_BITRATE_KBPS);
        assert!(estimate.total_kbps >= estimate.video_kbps);

        // Hardware encoder should have lower or equal initial video bitrate
        // (may be equal when both hit MIN_VIDEO_BITRATE_KBPS floor)
        let hw_estimate = estimate_export_bitrates(1, 20.0, true, 128, 2, true);
        assert!(hw_estimate.video_kbps <= estimate.video_kbps);
    }

    #[test]
    fn amf_initial_estimate_targets_the_90_to_95_percent_band() {
        let estimate =
            estimate_export_bitrates_for_encoder(3, 84.0, false, 0, 1, ExportVideoEncoder::HevcAmf);
        // AMF uses fill ratio 0.925 and efficiency 0.96
        // Generic hw uses fill ratio 1.0 and efficiency 0.50
        // The lower fill ratio reduces target size, so AMF estimate may be lower
        // The key assertion is that AMF estimate lands in the target band
        assert!(
            (240..=290).contains(&estimate.video_kbps),
            "AMF estimate {} should be in 240-290 range",
            estimate.video_kbps
        );
        assert_eq!(
            desired_output_size_bytes(ExportVideoEncoder::HevcAmf, target_size_bytes(3)),
            ((target_size_bytes(3) as f64) * AMF_TARGET_FILL_RATIO_IDEAL).round() as u64
        );
    }

    #[test]
    fn ordered_hardware_encoder_preferences_respects_auto_and_explicit_requests() {
        assert_eq!(
            ordered_hardware_encoder_preferences(EncoderType::Auto),
            vec![
                ExportVideoEncoder::HevcAmf,
                ExportVideoEncoder::HevcNvenc,
                ExportVideoEncoder::HevcQsv
            ]
        );
        assert_eq!(
            ordered_hardware_encoder_preferences(EncoderType::Nvenc),
            vec![
                ExportVideoEncoder::HevcNvenc,
                ExportVideoEncoder::HevcAmf,
                ExportVideoEncoder::HevcQsv
            ]
        );
        assert_eq!(
            ordered_hardware_encoder_preferences(EncoderType::Amf),
            vec![
                ExportVideoEncoder::HevcAmf,
                ExportVideoEncoder::HevcNvenc,
                ExportVideoEncoder::HevcQsv
            ]
        );
        assert_eq!(
            ordered_hardware_encoder_preferences(EncoderType::Qsv),
            vec![
                ExportVideoEncoder::HevcQsv,
                ExportVideoEncoder::HevcAmf,
                ExportVideoEncoder::HevcNvenc
            ]
        );
    }

    #[test]
    fn initial_high_bitrate_is_encoder_specific_and_bounded() {
        assert_eq!(
            ExportVideoEncoder::HevcNvenc.initial_high_bitrate_kbps(2_000),
            None
        );
        assert_eq!(
            ExportVideoEncoder::HevcQsv.initial_high_bitrate_kbps(2_000),
            None
        );

        assert_eq!(
            ExportVideoEncoder::SoftwareHevc.initial_high_bitrate_kbps(1_000),
            Some(1_550)
        );
        assert_eq!(
            ExportVideoEncoder::HevcAmf.initial_high_bitrate_kbps(1_000),
            Some(1_750)
        );

        // Even very low initial values should get at least +100 kbps headroom.
        assert_eq!(
            ExportVideoEncoder::SoftwareHevc.initial_high_bitrate_kbps(100),
            Some(200)
        );
        assert_eq!(
            ExportVideoEncoder::HevcAmf.initial_high_bitrate_kbps(100),
            Some(200)
        );

        let near_cap = MAX_VIDEO_BITRATE_KBPS - 100;
        assert_eq!(
            ExportVideoEncoder::SoftwareHevc.initial_high_bitrate_kbps(near_cap),
            Some(MAX_VIDEO_BITRATE_KBPS)
        );
        assert_eq!(
            ExportVideoEncoder::HevcAmf.initial_high_bitrate_kbps(near_cap),
            Some(MAX_VIDEO_BITRATE_KBPS)
        );
    }

    #[test]
    fn next_export_video_bitrate_uses_stronger_reduction_for_large_overshoot() {
        let moderate_overshoot_next = next_export_video_bitrate_kbps(
            ExportVideoEncoder::SoftwareHevc,
            4_000,
            4_600_000,
            4_000_000,
            0,
            MIN_VIDEO_BITRATE_KBPS,
            Some(3_900),
        );
        let high_overshoot_next = next_export_video_bitrate_kbps(
            ExportVideoEncoder::SoftwareHevc,
            4_000,
            5_200_000,
            4_000_000,
            0,
            MIN_VIDEO_BITRATE_KBPS,
            Some(3_900),
        );

        assert!(high_overshoot_next < moderate_overshoot_next);
        assert!((100..=3_900).contains(&moderate_overshoot_next));
        assert!((100..=3_900).contains(&high_overshoot_next));
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

        assert!((260..=264).contains(&next_video_bitrate_kbps));
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
            stream_copy: false,
            output_width: None,
            output_height: None,
            output_fps: None,
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
}
