use crate::config::EncoderType;
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

/// A single calibration encode data point: sample encoded at a specific video bitrate.
#[derive(Debug, Clone, Copy)]
struct CalibrationPoint {
    video_bitrate_kbps: u32,
    total_output_bytes: u64,
    sample_duration_secs: f64,
    num_sample_segments: usize,
}

impl CalibrationPoint {
    /// Estimated video-only bytes (total minus audio and container overhead).
    fn video_bytes(&self, audio_bitrate_kbps: u32) -> f64 {
        let non_video = estimate_non_video_bytes(
            self.sample_duration_secs,
            audio_bitrate_kbps,
            self.num_sample_segments,
        );
        (self.total_output_bytes.saturating_sub(non_video).max(1)) as f64
    }

    /// Video bytes per second at this bitrate.
    fn video_bytes_per_sec(&self, audio_bitrate_kbps: u32) -> f64 {
        self.video_bytes(audio_bitrate_kbps) / self.sample_duration_secs.max(0.1)
    }
}

/// Two-point calibration enabling power-law interpolation across the encoder's
/// bitrate-to-output response curve. Encodes the same representative sample at
/// two bracketing bitrates, then fits `vbps = a × B^p` and inverts to find the
/// bitrate that produces the desired video bytes.
#[derive(Debug, Clone)]
struct TwoPointCalibration {
    low: CalibrationPoint,
    high: CalibrationPoint,
    audio_bitrate_kbps: u32,
}

impl TwoPointCalibration {
    /// Interpolate using a power-law model to find the video bitrate that should
    /// produce `target_video_bytes` over `full_duration_secs`.
    ///
    /// A safety margin < 1.0 biases the result toward undershoot to avoid
    /// exceeding the file size target.
    fn interpolate_bitrate(
        &self,
        target_video_bytes: u64,
        full_duration_secs: f64,
        safety_margin: f64,
    ) -> u32 {
        let safe_target = (target_video_bytes as f64) * safety_margin.clamp(0.5, 1.0);
        let target_vbps = safe_target / full_duration_secs.max(0.1);
        let low_vbps = self.low.video_bytes_per_sec(self.audio_bitrate_kbps);
        let high_vbps = self.high.video_bytes_per_sec(self.audio_bitrate_kbps);
        let low_br = self.low.video_bitrate_kbps as f64;
        let high_br = self.high.video_bitrate_kbps as f64;

        info!(
            low_bitrate_kbps = self.low.video_bitrate_kbps,
            high_bitrate_kbps = self.high.video_bitrate_kbps,
            low_video_bps = format!("{:.1}", low_vbps),
            high_video_bps = format!("{:.1}", high_vbps),
            target_video_bps = format!("{:.1}", target_vbps),
            safety_margin,
            "Two-point calibration interpolation inputs"
        );

        // Guard: degenerate calibration data
        if low_vbps <= 0.0 || high_vbps <= 0.0 || low_br <= 0.0 || high_br <= 0.0 {
            warn!("Calibration points have invalid values, using midpoint fallback");
            return ((low_br + high_br) / 2.0).round().clamp(
                MIN_AUTO_VIDEO_BITRATE_KBPS as f64,
                MAX_VIDEO_BITRATE_KBPS as f64,
            ) as u32;
        }

        // Guard: bitrates are too close together for meaningful interpolation
        if (high_br / low_br - 1.0).abs() < 0.05 {
            let avg_bps = (low_vbps + high_vbps) / 2.0;
            let scale = if avg_bps > 0.0 {
                target_vbps / avg_bps
            } else {
                1.0
            };
            return ((low_br + high_br) / 2.0 * scale).round().clamp(
                MIN_AUTO_VIDEO_BITRATE_KBPS as f64,
                MAX_VIDEO_BITRATE_KBPS as f64,
            ) as u32;
        }

        // Power-law model: vbps = a × B^p
        // Solve for p from two points: p = ln(vbps_high / vbps_low) / ln(B_high / B_low)
        let p = ((high_vbps / low_vbps).ln() / (high_br / low_br).ln()).clamp(
            CALIBRATION_POWER_EXPONENT_MIN,
            CALIBRATION_POWER_EXPONENT_MAX,
        );

        // Solve for a: a = vbps_low / B_low^p
        let a = low_vbps / low_br.powf(p);

        // Invert: B_target = (target_vbps / a)^(1/p)
        let b_target = if a > 0.0 {
            (target_vbps / a).powf(1.0 / p)
        } else {
            (low_br + high_br) / 2.0
        };

        info!(
            power_exponent = format!("{:.4}", p),
            coefficient_a = format!("{:.4}", a),
            interpolated_bitrate_kbps = format!("{:.1}", b_target),
            "Two-point calibration power-law fit"
        );

        b_target.round().clamp(
            MIN_AUTO_VIDEO_BITRATE_KBPS as f64,
            MAX_VIDEO_BITRATE_KBPS as f64,
        ) as u32
    }
}

const MIN_VIDEO_BITRATE_KBPS: u32 = 300;
const MIN_AUTO_VIDEO_BITRATE_KBPS: u32 = 100;
const MAX_EXPORT_ATTEMPTS: usize = 6;
const AMF_MAX_EXPORT_ATTEMPTS: usize = 4;
const MIN_REASONABLE_FPS: f64 = 1.0;
const MAX_REASONABLE_FPS: f64 = 240.0;
const FALLBACK_EXPORT_FPS: f64 = 60.0;
const AMF_TARGET_FILL_RATIO_MIN: f64 = 0.90;
const AMF_TARGET_FILL_RATIO_IDEAL: f64 = 0.925;
const TARGET_SIZE_UNDERFILL_RATIO: f64 = 0.992;
const MAX_VIDEO_BITRATE_KBPS: u32 = 400_000;

// --- Two-point calibration constants ---
// Minimum clip duration (seconds) to enable calibration. Shorter clips use the
// naive bitrate estimate directly since calibration overhead is not worthwhile.
const CALIBRATION_MIN_CLIP_DURATION_SECS: f64 = 8.0;
// Fraction of clip duration used for the calibration sample (each encode).
const CALIBRATION_SAMPLE_RATIO: f64 = 0.25;
const CALIBRATION_MIN_SAMPLE_SECS: f64 = 5.0;
const CALIBRATION_MAX_SAMPLE_SECS: f64 = 20.0;
// Bracketing factors applied to the naive bitrate estimate. The low and high
// points should straddle the expected final bitrate to give the power-law model
// a meaningful spread.
const CALIBRATION_LOW_BITRATE_FACTOR: f64 = 0.60;
const CALIBRATION_HIGH_BITRATE_FACTOR: f64 = 1.60;
// Safety margin applied after interpolation to prefer a slight undershoot over
// any overshoot. 0.97 = target 97% fill.
const CALIBRATION_SAFETY_MARGIN: f64 = 0.97;
// Bounds for the power-law exponent p in `vbps = a × B^p`. Values outside
// this range indicate degenerate encoder behaviour; clamping keeps the model
// well-behaved.
const CALIBRATION_POWER_EXPONENT_MIN: f64 = 0.3;
const CALIBRATION_POWER_EXPONENT_MAX: f64 = 2.0;

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

    fn supports_calibration(self) -> bool {
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
                super::sdk_export::run_stream_copy_export_sdk(request, progress_tx, cancel_flag)?;
            info!(
                elapsed_secs = export_started_at.elapsed().as_secs_f64(),
                output = ?request.output_path,
                "Stream copy export path completed"
            );
            return Ok(stream_copy_result);
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

    // --- Two-point calibration ---
    // Encode a representative sample at two bracketing bitrates, fit a power-law
    // model to the encoder's response curve, and interpolate to find the bitrate
    // that should hit the target file size in a single full encode.
    if selected_encoder.supports_calibration()
        && output_duration_secs >= CALIBRATION_MIN_CLIP_DURATION_SECS
    {
        #[cfg(not(feature = "ffmpeg"))]
        {
            anyhow::bail!("Calibration requires ffmpeg feature");
        }

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

        let cal_sample_request =
            build_calibration_sample_request(request, export_work_dir.join("cal-sample.mp4"));
        let sample_duration = cal_sample_request.output_duration_secs().max(0.1);
        let sample_num_segments = cal_sample_request.keep_ranges.len();
        let naive_bitrate = bitrate_estimate
            .video_kbps
            .max(selected_encoder.min_video_bitrate_kbps());
        let low_bitrate = ((naive_bitrate as f64) * CALIBRATION_LOW_BITRATE_FACTOR)
            .round()
            .max(selected_encoder.min_video_bitrate_kbps() as f64) as u32;
        let high_bitrate = ((naive_bitrate as f64) * CALIBRATION_HIGH_BITRATE_FACTOR)
            .round()
            .min(MAX_VIDEO_BITRATE_KBPS as f64) as u32;

        info!(
            naive_bitrate_kbps = naive_bitrate,
            low_cal_bitrate_kbps = low_bitrate,
            high_cal_bitrate_kbps = high_bitrate,
            sample_duration_secs = sample_duration,
            sample_segments = sample_num_segments,
            "Starting two-point calibration"
        );

        // --- Low-point encode ---
        let low_path = export_work_dir.join("cal-low.mp4");
        let low_result = match super::sdk_export::attempt_export(
            &cal_sample_request,
            &low_path,
            low_bitrate,
            bitrate_estimate.audio_kbps,
            selected_encoder,
            progress_tx,
            cancel_flag,
            0,
            2,
            ClipExportPhase::Calibration,
        ) {
            Ok(Some(result)) => Some(result),
            Ok(None) => {
                let _ = std::fs::remove_file(&low_path);
                return Ok(ExportOutcome::Cancelled);
            }
            Err(err) if should_fallback_to_software_encoder(&err) => {
                warn!(
                    encoder = selected_encoder.ffmpeg_name(),
                    "Calibration low-point failed, falling back to libx265: {err:#}"
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
                let _ = std::fs::remove_file(&low_path);
                None
            }
            Err(err) => {
                let _ = std::fs::remove_file(&low_path);
                return Err(err);
            }
        };

        // --- High-point encode (only if low-point succeeded and encoder unchanged) ---
        if let Some(low_attempt) = low_result {
            if selected_encoder.supports_calibration() {
                let high_path = export_work_dir.join("cal-high.mp4");
                match super::sdk_export::attempt_export(
                    &cal_sample_request,
                    &high_path,
                    high_bitrate,
                    bitrate_estimate.audio_kbps,
                    selected_encoder,
                    progress_tx,
                    cancel_flag,
                    1,
                    2,
                    ClipExportPhase::Calibration,
                ) {
                    Ok(Some(high_attempt)) => {
                        let calibration = TwoPointCalibration {
                            low: CalibrationPoint {
                                video_bitrate_kbps: low_bitrate,
                                total_output_bytes: low_attempt.size_bytes,
                                sample_duration_secs: sample_duration,
                                num_sample_segments: sample_num_segments,
                            },
                            high: CalibrationPoint {
                                video_bitrate_kbps: high_bitrate,
                                total_output_bytes: high_attempt.size_bytes,
                                sample_duration_secs: sample_duration,
                                num_sample_segments: sample_num_segments,
                            },
                            audio_bitrate_kbps: bitrate_estimate.audio_kbps,
                        };
                        let full_non_video = estimate_non_video_bytes(
                            output_duration_secs,
                            bitrate_estimate.audio_kbps,
                            request.keep_ranges.len(),
                        );
                        let target_video_bytes =
                            target_size_bytes.saturating_sub(full_non_video).max(1);
                        let calibrated = calibration.interpolate_bitrate(
                            target_video_bytes,
                            output_duration_secs,
                            CALIBRATION_SAFETY_MARGIN,
                        );
                        info!(
                            calibrated_bitrate_kbps = calibrated,
                            low_output_bytes = low_attempt.size_bytes,
                            high_output_bytes = high_attempt.size_bytes,
                            target_video_bytes,
                            full_non_video_bytes = full_non_video,
                            "Two-point calibration complete, using interpolated bitrate"
                        );
                        bitrate_estimate.video_kbps = calibrated;
                    }
                    Ok(None) => {
                        let _ = std::fs::remove_file(&high_path);
                        let _ = std::fs::remove_file(&low_path);
                        return Ok(ExportOutcome::Cancelled);
                    }
                    Err(err) if should_fallback_to_software_encoder(&err) => {
                        warn!(
                            encoder = selected_encoder.ffmpeg_name(),
                            "Calibration high-point failed, falling back to libx265: {err:#}"
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
                    }
                    Err(err) => {
                        let _ = std::fs::remove_file(&high_path);
                        let _ = std::fs::remove_file(&low_path);
                        return Err(err);
                    }
                }
                let _ = std::fs::remove_file(export_work_dir.join("cal-high.mp4"));
            }
        }
        let _ = std::fs::remove_file(export_work_dir.join("cal-low.mp4"));
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
                Err(e) => {
                    warn!(
                        "Failed to probe exported output for validity checks: {:?}: {}",
                        request.output_path, e
                    );
                    // Continue - don't fail the export just because post-hoc validation failed
                }
            }
        }

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

#[allow(clippy::too_many_arguments)]
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

#[allow(clippy::too_many_arguments)]
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
            // Apply a stronger correction if we missed our adaptive target bounds
            ratio *= 0.95;
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

    next_video_bitrate_kbps.clamp(100, MAX_VIDEO_BITRATE_KBPS)
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

fn calibration_sample_duration_secs(output_duration_secs: f64) -> f64 {
    output_duration_secs
        .mul_add(CALIBRATION_SAMPLE_RATIO, 0.0)
        .clamp(CALIBRATION_MIN_SAMPLE_SECS, CALIBRATION_MAX_SAMPLE_SECS)
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

/// Build a calibration sample request with 3 evenly-spaced windows across the
/// clip for representative content coverage. Both calibration encodes use the
/// same sample request (only the bitrate differs).
fn build_calibration_sample_request(
    request: &ClipExportRequest,
    output_path: PathBuf,
) -> ClipExportRequest {
    let full_duration = request.output_duration_secs().max(0.1);
    let sample_duration = calibration_sample_duration_secs(full_duration);

    // 3 windows for good content coverage; fewer for very short samples
    let num_windows = if sample_duration >= 3.0 {
        3
    } else if sample_duration >= 1.5 {
        2
    } else {
        1
    };
    let window_duration = (sample_duration / num_windows as f64)
        .max(0.5)
        .min(full_duration);

    // Spread windows evenly: start, middle, end
    let window_starts: Vec<f64> = if num_windows == 1 {
        vec![((full_duration - window_duration) / 2.0).max(0.0)]
    } else {
        let max_start = (full_duration - window_duration).max(0.0);
        (0..num_windows)
            .map(|i| max_start * (i as f64) / ((num_windows - 1) as f64).max(1.0))
            .collect()
    };

    let mut keep_ranges = Vec::new();
    for start in &window_starts {
        keep_ranges.extend(slice_keep_ranges_for_output_window(
            &request.keep_ranges,
            *start,
            window_duration,
        ));
    }

    let mut cal_request = request.clone();
    cal_request.output_path = output_path;
    cal_request.keep_ranges = keep_ranges;
    cal_request
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
        // Software and Auto don't prefer any specific hardware encoder
        EncoderType::Software | EncoderType::Auto => {}
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quality_contracts::{
        validate_export_validity, ExportValidationInput, ExportValidityViolation,
    };

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

        // ideal = 3.14MB * 0.925 = ~2.9MB
        // current = 3.22MB
        // ratio = 2.9 / 3.22 = ~0.90
        // with the new 0.95 overshoot multiplier: ratio = 0.90 * 0.95 = ~0.855
        // 300 * 0.855 = 256.5
        assert!(
            (254..=258).contains(&next_video_bitrate_kbps),
            "next_video_bitrate_kbps: {}",
            next_video_bitrate_kbps
        );
    }

    #[test]
    fn calibration_sample_builds_three_windows_for_long_clips() {
        let request = ClipExportRequest {
            input_path: PathBuf::from("input.mp4"),
            output_path: PathBuf::from("output.mp4"),
            keep_ranges: vec![TimeRange {
                start_secs: 0.0,
                end_secs: 120.0,
            }],
            target_size_mb: 3,
            audio_bitrate_kbps: 128,
            use_hardware_acceleration: true,
            preferred_encoder: EncoderType::Auto,
            metadata: VideoFileMetadata {
                duration_secs: 120.0,
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

        let cal = build_calibration_sample_request(&request, PathBuf::from("cal.mp4"));
        // 120s × 0.25 = 30s, clamped to max 20s → 3 windows of ~6.67s each
        let sample_dur = cal.output_duration_secs();
        assert!(
            sample_dur >= 19.0 && sample_dur <= 21.0,
            "sample_dur: {sample_dur}"
        );
        assert_eq!(cal.keep_ranges.len(), 3);
        // First window starts at the beginning
        assert!(cal.keep_ranges[0].start_secs < 1.0);
        // Last window reaches near the end
        let last = cal.keep_ranges.last().unwrap();
        assert!(last.end_secs > 115.0, "last.end_secs: {}", last.end_secs);
    }

    #[test]
    fn calibration_sample_slices_across_multi_range_clip() {
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

        let cal = build_calibration_sample_request(&request, PathBuf::from("cal.mp4"));
        // Output duration is 15s; 15s × 0.25 = 3.75s → clamped to min 5s
        let sample_dur = cal.output_duration_secs();
        assert!(
            sample_dur >= 4.5 && sample_dur <= 5.5,
            "sample_dur: {sample_dur}"
        );
        // Should have sliced into the keep_ranges
        assert!(!cal.keep_ranges.is_empty());
        for range in &cal.keep_ranges {
            assert!(range.duration_secs() > 0.0);
        }
    }

    #[test]
    fn two_point_interpolation_finds_correct_bitrate_for_linear_encoder() {
        // Simulate a perfectly linear encoder: doubling bitrate doubles output
        // Low: 200 kbps → 25000 bytes/sec for 10s = 250000 bytes video
        // High: 400 kbps → 50000 bytes/sec for 10s = 500000 bytes video
        let audio_kbps = 0;
        let sample_secs = 10.0;
        let overhead = estimate_non_video_bytes(sample_secs, audio_kbps, 1);
        let cal = TwoPointCalibration {
            low: CalibrationPoint {
                video_bitrate_kbps: 200,
                total_output_bytes: 250_000 + overhead,
                sample_duration_secs: sample_secs,
                num_sample_segments: 1,
            },
            high: CalibrationPoint {
                video_bitrate_kbps: 400,
                total_output_bytes: 500_000 + overhead,
                sample_duration_secs: sample_secs,
                num_sample_segments: 1,
            },
            audio_bitrate_kbps: audio_kbps,
        };

        // Want 37500 bytes/sec for 60s = 2250000 video bytes with 1.0 safety
        let target_video_bytes = 2_250_000;
        let full_duration = 60.0;
        let result = cal.interpolate_bitrate(target_video_bytes, full_duration, 1.0);
        // Linear encoder: 37500 bps / 25000 bps_per_kbps_at_200 → 300 kbps
        assert!((295..=305).contains(&result), "expected ~300, got {result}");
    }

    #[test]
    fn two_point_interpolation_handles_sublinear_encoder() {
        // Simulate diminishing returns: 2× bitrate → 1.6× output
        // Low: 200 kbps → 20000 bytes/sec
        // High: 400 kbps → 32000 bytes/sec (1.6× not 2×)
        let audio_kbps = 48;
        let sample_secs = 10.0;
        let overhead = estimate_non_video_bytes(sample_secs, audio_kbps, 2);
        let cal = TwoPointCalibration {
            low: CalibrationPoint {
                video_bitrate_kbps: 200,
                total_output_bytes: 200_000 + overhead,
                sample_duration_secs: sample_secs,
                num_sample_segments: 2,
            },
            high: CalibrationPoint {
                video_bitrate_kbps: 400,
                total_output_bytes: 320_000 + overhead,
                sample_duration_secs: sample_secs,
                num_sample_segments: 2,
            },
            audio_bitrate_kbps: audio_kbps,
        };

        // Target between the two points: 25000 video bytes/sec for 60s
        let target_video_bytes = 1_500_000;
        let full_duration = 60.0;
        let result = cal.interpolate_bitrate(target_video_bytes, full_duration, 1.0);
        // Should be between 200 and 400
        assert!(
            (200..=400).contains(&result),
            "expected 200-400, got {result}"
        );
        // With sublinear response, getting 25000 bps needs more than a simple
        // midpoint (275) since the encoder's output grows slower than bitrate
        assert!(
            result > 270,
            "sublinear encoder should need higher bitrate than linear midpoint, got {result}"
        );
    }

    #[test]
    fn two_point_interpolation_extrapolates_below_low_point() {
        // Target is below what the low-point produced
        let audio_kbps = 0;
        let sample_secs = 10.0;
        let overhead = estimate_non_video_bytes(sample_secs, audio_kbps, 1);
        let cal = TwoPointCalibration {
            low: CalibrationPoint {
                video_bitrate_kbps: 300,
                total_output_bytes: 300_000 + overhead,
                sample_duration_secs: sample_secs,
                num_sample_segments: 1,
            },
            high: CalibrationPoint {
                video_bitrate_kbps: 600,
                total_output_bytes: 600_000 + overhead,
                sample_duration_secs: sample_secs,
                num_sample_segments: 1,
            },
            audio_bitrate_kbps: audio_kbps,
        };

        // Target much lower than what 300 kbps produces
        let target_video_bytes = 600_000; // 10000 bps for 60s
        let full_duration = 60.0;
        let result = cal.interpolate_bitrate(target_video_bytes, full_duration, 1.0);
        assert!(
            result < 300,
            "should extrapolate below low point, got {result}"
        );
        assert!(
            result >= MIN_AUTO_VIDEO_BITRATE_KBPS,
            "should stay above minimum, got {result}"
        );
    }

    #[test]
    fn two_point_interpolation_applies_safety_margin() {
        let audio_kbps = 0;
        let sample_secs = 10.0;
        let overhead = estimate_non_video_bytes(sample_secs, audio_kbps, 1);
        let cal = TwoPointCalibration {
            low: CalibrationPoint {
                video_bitrate_kbps: 200,
                total_output_bytes: 250_000 + overhead,
                sample_duration_secs: sample_secs,
                num_sample_segments: 1,
            },
            high: CalibrationPoint {
                video_bitrate_kbps: 400,
                total_output_bytes: 500_000 + overhead,
                sample_duration_secs: sample_secs,
                num_sample_segments: 1,
            },
            audio_bitrate_kbps: audio_kbps,
        };

        let target_video_bytes = 2_250_000;
        let full_duration = 60.0;
        let no_margin = cal.interpolate_bitrate(target_video_bytes, full_duration, 1.0);
        let with_margin = cal.interpolate_bitrate(target_video_bytes, full_duration, 0.97);
        assert!(
            with_margin < no_margin,
            "safety margin should reduce bitrate: no_margin={no_margin}, with_margin={with_margin}"
        );
    }

    #[test]
    fn calibration_sample_duration_scales_with_clip_length() {
        // Short clip: clamps to minimum
        assert!(
            (calibration_sample_duration_secs(10.0) - 5.0).abs() < 0.01,
            "10s clip should use 5s sample"
        );
        // Medium clip: 25%
        let dur_40 = calibration_sample_duration_secs(40.0);
        assert!(
            (dur_40 - 10.0).abs() < 0.01,
            "40s clip should use ~10s sample, got {dur_40}"
        );
        // Long clip: clamped to max
        let dur_200 = calibration_sample_duration_secs(200.0);
        assert!(
            (dur_200 - 20.0).abs() < 0.01,
            "200s clip should use 20s sample, got {dur_200}"
        );
        // Very short clip: can't exceed clip duration
        let dur_3 = calibration_sample_duration_secs(3.0);
        assert!(
            (dur_3 - 3.0).abs() < 0.01,
            "3s clip should use 3s sample, got {dur_3}"
        );
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

    #[test]
    fn export_contract_accepts_probe_metadata_for_nominal_export() {
        let metadata = VideoFileMetadata {
            duration_secs: 9.8,
            width: 1920,
            height: 1080,
            has_audio: true,
            fps: 60.0,
        };
        let violations = validate_export_validity(ExportValidationInput {
            expected_duration_secs: 10.0,
            expect_audio: true,
            metadata: &metadata,
        });
        assert!(violations.is_empty(), "violations: {violations:?}");
    }

    #[test]
    fn export_contract_flags_invalid_probe_metadata() {
        let metadata = VideoFileMetadata {
            duration_secs: 7.5,
            width: 0,
            height: 1080,
            has_audio: false,
            fps: 0.5,
        };
        let violations = validate_export_validity(ExportValidationInput {
            expected_duration_secs: 10.0,
            expect_audio: true,
            metadata: &metadata,
        });
        assert!(violations
            .iter()
            .any(|v| matches!(v, ExportValidityViolation::InvalidResolution { .. })));
        assert!(violations
            .iter()
            .any(|v| matches!(v, ExportValidityViolation::DurationTooShort { .. })));
        assert!(violations
            .iter()
            .any(|v| matches!(v, ExportValidityViolation::MissingAudioTrack)));
        assert!(violations
            .iter()
            .any(|v| matches!(v, ExportValidityViolation::InvalidFps { .. })));
    }
}
