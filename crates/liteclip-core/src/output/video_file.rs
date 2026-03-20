use anyhow::{bail, Context, Result};
use image::RgbaImage;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::Sender,
    Arc,
};
use std::thread;
use tracing::info;

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
    pub metadata: VideoFileMetadata,
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
const MAX_EXPORT_ATTEMPTS: usize = 4;
const TARGET_SIZE_UNDERFILL_RATIO: f64 = 0.985;
const MAX_VIDEO_BITRATE_KBPS: u32 = 400_000;

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
    let bitrate_estimate = estimate_export_bitrates(
        request.target_size_mb,
        output_duration_secs,
        request.metadata.has_audio,
        request.audio_bitrate_kbps,
        request.keep_ranges.len(),
    );
    let target_size_bytes = target_size_bytes(request.target_size_mb);
    let non_video_bytes = estimate_non_video_bytes(
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

    let export_result = (|| -> Result<ExportOutcome> {
        let mut current_video_bitrate_kbps =
            bitrate_estimate.video_kbps.max(MIN_VIDEO_BITRATE_KBPS);
        let mut low_video_bitrate_kbps = MIN_VIDEO_BITRATE_KBPS;
        let mut high_video_bitrate_kbps: Option<u32> = None;
        let mut best_under_target: Option<ExportAttemptResult> = None;
        let mut best_over_target: Option<ExportAttemptResult> = None;

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
                progress_tx,
                cancel_flag,
                attempt_index,
                MAX_EXPORT_ATTEMPTS,
            )? {
                Some(attempt_result) => attempt_result,
                None => return Ok(ExportOutcome::Cancelled),
            };

            info!(
                "Export attempt {}/{} produced {} bytes at {} kbps (target {} bytes)",
                attempt_index + 1,
                MAX_EXPORT_ATTEMPTS,
                attempt_result.size_bytes,
                attempt_result.video_bitrate_kbps,
                target_size_bytes
            );

            if attempt_result.size_bytes <= target_size_bytes {
                let replace_best_under = match best_under_target.as_ref() {
                    Some(best_attempt) => {
                        attempt_result.size_bytes > best_attempt.size_bytes
                            || (attempt_result.size_bytes == best_attempt.size_bytes
                                && attempt_result.video_bitrate_kbps
                                    > best_attempt.video_bitrate_kbps)
                    }
                    None => true,
                };
                if replace_best_under {
                    best_under_target = Some(attempt_result.clone());
                }
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
                high_video_bitrate_kbps = Some(match high_video_bitrate_kbps {
                    Some(high_bitrate) => high_bitrate.min(current_video_bitrate_kbps),
                    None => current_video_bitrate_kbps,
                });
            }

            if attempt_result.size_bytes <= target_size_bytes
                && (attempt_result.size_bytes as f64)
                    >= (target_size_bytes as f64 * TARGET_SIZE_UNDERFILL_RATIO)
            {
                break;
            }

            let next_video_bitrate_kbps = next_export_video_bitrate_kbps(
                current_video_bitrate_kbps,
                attempt_result.size_bytes,
                target_size_bytes,
                non_video_bytes,
                low_video_bitrate_kbps,
                high_video_bitrate_kbps,
            );

            if next_video_bitrate_kbps == current_video_bitrate_kbps {
                break;
            }

            if let Some(high_bitrate) = high_video_bitrate_kbps {
                if high_bitrate <= low_video_bitrate_kbps.saturating_add(24) {
                    break;
                }
            }

            current_video_bitrate_kbps = next_video_bitrate_kbps;
        }

        let preferred_attempt =
            select_preferred_attempt(best_under_target, best_over_target, target_size_bytes)
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

fn attempt_export(
    request: &ClipExportRequest,
    output_path: &Path,
    video_bitrate_kbps: u32,
    audio_bitrate_kbps: u32,
    progress_tx: &Sender<ClipExportUpdate>,
    cancel_flag: &Arc<AtomicBool>,
    attempt_index: usize,
    attempt_count: usize,
) -> Result<Option<ExportAttemptResult>> {
    let first_pass_filter = build_filter_complex(&request.keep_ranges, false);
    let second_pass_filter = build_filter_complex(&request.keep_ranges, request.metadata.has_audio);
    let ffmpeg = ffmpeg_executable_path();
    let passlog_prefix = output_path.with_extension("passlog");
    let passlog_prefix_str = passlog_prefix.to_string_lossy().into_owned();

    let preset = select_adaptive_preset(video_bitrate_kbps, &request.metadata);

    let _ = progress_tx.send(ClipExportUpdate::Progress {
        phase: ClipExportPhase::Preparing,
        fraction: 0.0,
        message: "Preparing export".to_string(),
    });

    info!(
        "Exporting clipped video {:?} -> {:?} ({} kept ranges, target={} MB, video bitrate={} kbps, preset={})",
        request.input_path,
        output_path,
        request.keep_ranges.len(),
        request.target_size_mb,
        video_bitrate_kbps,
        preset
    );

    let first_pass_args = build_ffmpeg_args(
        request,
        &first_pass_filter,
        &passlog_prefix_str,
        video_bitrate_kbps,
        audio_bitrate_kbps,
        true,
        &preset,
        output_path,
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
        &passlog_prefix_str,
        video_bitrate_kbps,
        audio_bitrate_kbps,
        false,
        &preset,
        output_path,
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

    let size_bytes = std::fs::metadata(output_path)
        .with_context(|| format!("Failed to get size of export output file {:?}", output_path))?
        .len();

    cleanup_passlog_files(&passlog_prefix);

    Ok(Some(ExportAttemptResult {
        output_path: output_path.to_path_buf(),
        video_bitrate_kbps,
        size_bytes,
    }))
}

fn build_ffmpeg_args(
    request: &ClipExportRequest,
    filter_complex: &str,
    passlog_prefix: &str,
    video_bitrate_kbps: u32,
    audio_bitrate_kbps: u32,
    first_pass: bool,
    preset: &str,
    output_path: &Path,
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
        "-filter_complex".to_string(),
        filter_complex.to_string(),
        "-map".to_string(),
        "[outv]".to_string(),
        "-c:v".to_string(),
        "libx264".to_string(),
        "-preset".to_string(),
        preset.to_string(),
        "-pix_fmt".to_string(),
        "yuv420p".to_string(),
        "-b:v".to_string(),
        format!("{video_bitrate_kbps}k"),
        "-maxrate".to_string(),
        format!("{video_bitrate_kbps}k"),
        "-bufsize".to_string(),
        format!("{}k", video_bitrate_kbps.saturating_mul(2)),
        "-pass".to_string(),
        if first_pass { "1" } else { "2" }.to_string(),
        "-passlogfile".to_string(),
        passlog_prefix.to_string(),
    ];

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
            format!("{}k", audio_bitrate_kbps.max(64)),
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

fn build_filter_complex(keep_ranges: &[TimeRange], has_audio: bool) -> String {
    let mut filters = Vec::new();

    for (index, range) in keep_ranges.iter().enumerate() {
        filters.push(format!(
            "[0:v:0]trim=start={}:end={},setpts=PTS-STARTPTS[v{index}]",
            format_seconds_arg(range.start_secs),
            format_seconds_arg(range.end_secs),
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
) -> ExportBitrateEstimate {
    let duration_secs = output_duration_secs.max(0.1);
    let total_kbps = ((target_size_bytes(target_size_mb) as f64) * 8.0 / duration_secs / 1000.0)
        .max(f64::from(MIN_VIDEO_BITRATE_KBPS));
    let audio_kbps = select_audio_bitrate_kbps(has_audio, requested_audio_bitrate_kbps, total_kbps);
    let non_video_bytes = estimate_non_video_bytes(duration_secs, audio_kbps, num_segments);
    let video_kbps =
        ((((target_size_bytes(target_size_mb).saturating_sub(non_video_bytes)) as f64) * 8.0)
            / duration_secs
            / 1000.0)
            .max(f64::from(MIN_VIDEO_BITRATE_KBPS))
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

    let requested_audio_bitrate_kbps = requested_audio_bitrate_kbps.max(64);
    if total_bitrate_kbps < 900.0 {
        64
    } else if total_bitrate_kbps < 1800.0 {
        requested_audio_bitrate_kbps.min(96)
    } else {
        requested_audio_bitrate_kbps.min(128)
    }
}

fn next_export_video_bitrate_kbps(
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
    let target_video_bytes = target_size_bytes
        .saturating_sub(estimated_non_video_bytes)
        .max(1);
    let scaled_video_bitrate_kbps = ((f64::from(current_video_bitrate_kbps)
        * (target_video_bytes as f64)
        / (current_video_bytes as f64))
        * if actual_size_bytes > target_size_bytes {
            0.985
        } else {
            1.01
        })
    .round() as u32;

    let growth_limited_video_bitrate_kbps = scaled_video_bitrate_kbps.min(
        current_video_bitrate_kbps
            .saturating_mul(4)
            .max(MIN_VIDEO_BITRATE_KBPS),
    );
    let mut next_video_bitrate_kbps =
        growth_limited_video_bitrate_kbps.max(low_video_bitrate_kbps.max(MIN_VIDEO_BITRATE_KBPS));

    if let Some(high_video_bitrate_kbps) = high_video_bitrate_kbps {
        if high_video_bitrate_kbps <= low_video_bitrate_kbps.saturating_add(24) {
            return low_video_bitrate_kbps.max(MIN_VIDEO_BITRATE_KBPS);
        }

        next_video_bitrate_kbps = next_video_bitrate_kbps.min(high_video_bitrate_kbps);
        if next_video_bitrate_kbps == current_video_bitrate_kbps {
            let span = high_video_bitrate_kbps.saturating_sub(low_video_bitrate_kbps);
            next_video_bitrate_kbps = low_video_bitrate_kbps.saturating_add((span + 1) / 2);
        }
    }

    next_video_bitrate_kbps.min(MAX_VIDEO_BITRATE_KBPS)
}

fn select_preferred_attempt(
    best_under_target: Option<ExportAttemptResult>,
    best_over_target: Option<ExportAttemptResult>,
    target_size_bytes: u64,
) -> Option<ExportAttemptResult> {
    match (best_under_target, best_over_target) {
        (Some(under_target), Some(over_target)) => {
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

fn select_adaptive_preset(video_bitrate_kbps: u32, metadata: &VideoFileMetadata) -> &'static str {
    // Adaptive preset selection: use slower presets at lower bitrates
    // for better compression efficiency, and faster presets at high bitrates
    let pixels_per_frame = f64::from(metadata.width.max(1)) * f64::from(metadata.height.max(1));
    let bits_per_pixel_frame =
        (f64::from(video_bitrate_kbps) * 1000.0) / (pixels_per_frame * metadata.fps.max(1.0));
    let high_throughput_source = pixels_per_frame * metadata.fps.max(1.0) >= 2560.0 * 1440.0 * 60.0;

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

fn phase_label(phase: ClipExportPhase) -> &'static str {
    match phase {
        ClipExportPhase::Preparing => "Preparing export",
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

#[cfg(any(test, all(feature = "ffmpeg-cli", not(feature = "ffmpeg"))))]
fn command_output(command: &mut Command) -> Result<std::process::Output> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    Ok(command.output()?)
}

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
        let estimate = estimate_export_bitrates(1, 20.0, true, 128, 2);

        assert_eq!(estimate.audio_kbps, 64);
        assert!(estimate.video_kbps >= MIN_VIDEO_BITRATE_KBPS);
        assert!(estimate.total_kbps >= estimate.video_kbps);
    }

    #[test]
    fn next_export_video_bitrate_tracks_video_budget_instead_of_total_size() {
        let next_video_bitrate_kbps = next_export_video_bitrate_kbps(
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

    #[cfg(any(feature = "ffmpeg", feature = "ffmpeg-cli"))]
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
            "libx264",
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
                metadata,
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
