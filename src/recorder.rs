use log::{debug, error, info, warn};
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tempfile::TempDir;

use crate::settings::{RateControl, Resolution, Settings, VideoEncoder};

const SEGMENT_DURATION_SECS: u64 = 4;
const STABLE_SEGMENT_GUARD_MS: u128 = 1200;
const MIN_STABLE_SEGMENT_BYTES: u64 = 64 * 1024;

/// Cache for detected capabilities to avoid re-probing on every startup.
#[derive(Debug, Serialize, Deserialize)]
struct RecorderCache {
    /// Output of `ffmpeg -version` to identify the binary.
    ffmpeg_version_string: String,
    /// List of working video encoders.
    video_encoders: Vec<VideoEncoder>,
    /// List of detected audio devices.
    audio_devices: Vec<String>,
}

/// The input method used for screen capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScreenCaptureInput {
    /// Desktop Duplication API (high performance, Windows 8+)
    DdaGrab,
    /// GDI-based grab (compatibility fallback)
    GdiGrab,
}

/// Current state of the recorder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecorderState {
    /// Not recording
    Idle,
    /// Actively recording to rolling segments
    Recording,
    /// Currently concatenating segments into a final clip
    Saving,
}

/// Windows-only: create process without a visible console window.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

/// Build an FFmpeg `Command` with the console window suppressed on Windows.
fn ffmpeg_command() -> Command {
    let mut cmd = Command::new(resolve_ffmpeg_executable());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

fn resolve_ffmpeg_executable() -> PathBuf {
    if let Ok(explicit) = env::var("LITECLIP_FFMPEG") {
        let explicit_path = PathBuf::from(explicit);
        if explicit_path.is_file() {
            return explicit_path;
        }
    }

    let binary_name = if cfg!(windows) {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    };

    let mut candidates: Vec<PathBuf> = Vec::new();

    if let Ok(current_exe) = env::current_exe() {
        if let Some(exe_dir) = current_exe.parent() {
            candidates.push(exe_dir.join(binary_name));
            candidates.push(exe_dir.join("ffmpeg").join(binary_name));
            candidates.push(exe_dir.join("ffmpeg").join("bin").join(binary_name));
            candidates.push(exe_dir.join("bin").join(binary_name));
        }
    }

    if let Ok(cwd) = env::current_dir() {
        candidates.push(cwd.join(binary_name));
        candidates.push(cwd.join("ffmpeg").join(binary_name));
        candidates.push(cwd.join("ffmpeg").join("bin").join(binary_name));
    }

    for candidate in candidates {
        if candidate.is_file() {
            return candidate;
        }
    }

    PathBuf::from(binary_name)
}

/// Manages the FFmpeg subprocess and segment-based replay buffer.
///
/// The recorder uses FFmpeg to capture screen and audio into short rolling segments.
/// When a clip is saved, these segments are concatenated using FFmpeg's concat muxer.
pub struct Recorder {
    /// The current operational state of the recorder.
    pub state: RecorderState,
    /// User settings for quality, resolution, etc.
    pub settings: Settings,
    /// Whether FFmpeg was found on the system PATH.
    pub ffmpeg_found: bool,
    /// Path to the last successfully saved clip.
    pub last_saved_path: Option<PathBuf>,
    /// List of detected audio input devices.
    pub audio_devices: Vec<String>,
    /// List of available hardware/software video encoders.
    pub video_encoders: Vec<VideoEncoder>,
    screen_capture_input: ScreenCaptureInput,

    child: Option<Child>,
    temp_dir: Option<TempDir>,
    started_at: Option<Instant>,
    recording_expected: bool,
    restart_attempts: u32,
    next_restart_attempt_at: Option<Instant>,
}

struct SaveJob {
    resume_state: RecorderState,
    output_path: PathBuf,
    segment_paths: Vec<PathBuf>,
}

struct SaveExecution {
    resume_state: RecorderState,
    save_result: Result<PathBuf, String>,
}

impl Recorder {
    /// Create a new Recorder with default settings.
    pub fn new() -> Self {
        let resolved_ffmpeg = resolve_ffmpeg_executable();
        info!(
            "Using FFmpeg executable candidate: {}",
            resolved_ffmpeg.display()
        );

        // Check if FFmpeg is available on PATH
        let ffmpeg_found = ffmpeg_command()
            .arg("-version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false);

        if ffmpeg_found {
            info!("FFmpeg found on PATH");
        } else {
            error!("FFmpeg NOT found on PATH — recording will not work");
        }

        // Try to load cache
        let cache_dir = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("LiteClipReplay");
        let cache_path = cache_dir.join("recorder_cache.json");
        let ffmpeg_version = get_ffmpeg_version_string().unwrap_or_default();

        let (audio_devices, video_encoders) =
            if let Some(cache) = load_recorder_cache(&cache_path, &ffmpeg_version) {
                info!(
                    "Loaded capabilities from cache: {} audio devices, {} encoders",
                    cache.audio_devices.len(),
                    cache.video_encoders.len()
                );
                (cache.audio_devices, cache.video_encoders)
            } else {
                info!("Capabilities cache missing or invalid — probing system...");
                let audio_devices = detect_audio_devices();
                let video_encoders = detect_video_encoders(ffmpeg_found);

                if !ffmpeg_version.is_empty() {
                    save_recorder_cache(
                        &cache_path,
                        RecorderCache {
                            ffmpeg_version_string: ffmpeg_version,
                            video_encoders: video_encoders.clone(),
                            audio_devices: audio_devices.clone(),
                        },
                    );
                }
                (audio_devices, video_encoders)
            };

        info!("Final Audio Devices: {:?}", audio_devices);
        info!("Final Video Encoders: {:?}", video_encoders);

        let screen_capture_input = detect_screen_capture_input(ffmpeg_found);
        info!("Selected screen capture input: {:?}", screen_capture_input);

        let mut settings = Settings::load();

        // Performance safeguard: if Auto encoder resolves to software x264,
        // avoid high-resolution native capture by default to reduce dropped
        // frames on CPU-bound systems.
        let resolved_encoder = settings.video_encoder.resolve(&video_encoders);
        if settings.video_encoder == VideoEncoder::Auto
            && resolved_encoder == VideoEncoder::Libx264
            && !settings.custom_resolution_enabled
            && settings.resolution == Resolution::Native
        {
            settings.resolution = Resolution::Res1080p;
            settings.save();
            info!(
                "Auto encoder resolved to libx264; applied 1080p performance fallback to reduce capture lag"
            );
        }

        // If no audio device is saved, or the saved one is not in the detected list,
        // fallback to the first available device.
        if !audio_devices.is_empty() {
            let device_valid = settings
                .audio_device
                .as_ref()
                .map(|saved| audio_devices.contains(saved))
                .unwrap_or(false);

            if !device_valid {
                settings.audio_device = Some(audio_devices[0].clone());
                info!("Auto-selected audio device: {}", audio_devices[0]);
            }
        } else {
            settings.audio_device = None;
        }

        Self {
            state: RecorderState::Idle,
            settings,
            ffmpeg_found,
            last_saved_path: None,
            audio_devices,
            video_encoders,
            screen_capture_input,
            child: None,
            temp_dir: None,
            started_at: None,
            recording_expected: false,
            restart_attempts: 0,
            next_restart_attempt_at: None,
        }
    }

    /// How many segment files to keep in the ring buffer.
    fn segment_wrap(&self) -> u64 {
        let requested = self.settings.buffer_seconds.max(1);
        requested.div_ceil(SEGMENT_DURATION_SECS).max(1)
    }

    /// How long the buffer has been recording.
    pub fn elapsed_seconds(&self) -> u64 {
        self.started_at.map(|t| t.elapsed().as_secs()).unwrap_or(0)
    }

    /// Start recording the desktop into rolling segments.
    ///
    /// This launches an FFmpeg subprocess. It will attempt to use hardware encoders
    /// if available and falls back to software (libx264) if they fail.
    pub fn start(&mut self) -> Result<(), String> {
        if !self.ffmpeg_found {
            error!("Cannot start: FFmpeg not found on PATH");
            self.recording_expected = false;
            self.restart_attempts = 0;
            self.next_restart_attempt_at = None;
            return Err("FFmpeg not found on PATH. Please install FFmpeg.".into());
        }
        if self.state == RecorderState::Recording {
            warn!("Start called but already recording — ignoring");
            return Err("Already recording.".into());
        }
        self.recording_expected = true;

        info!(
            "Starting recording — encoder={:?}, quality={:?}, fps={:?}, resolution={:?}, buffer={}s, audio={}",
            self.settings.video_encoder,
            self.settings.quality,
            self.settings.framerate,
            self.settings.resolution,
            self.settings.buffer_seconds,
            self.settings.capture_audio,
        );

        // Create temp dir for segments
        let temp_dir = TempDir::new().map_err(|e| {
            error!("Failed to create temp dir: {}", e);
            format!("Failed to create temp dir: {}", e)
        })?;
        info!("Temp segment dir: {}", temp_dir.path().display());
        let segment_pattern = temp_dir.path().join("seg_%04d.ts");

        let segment_duration: u64 = SEGMENT_DURATION_SECS;
        let force_kf = format!("expr:gte(t,n_forced*{})", segment_duration);
        let fps = self.settings.framerate.value().to_string();
        let wrap = self.segment_wrap().to_string();
        let keyframe_interval_sec = self.settings.keyframe_interval_sec.max(1);
        let keyint_frames = (self.settings.framerate.value() * keyframe_interval_sec).to_string();
        // Keep at least one logical core free so desktop input/rendering stays responsive.
        // Use more than 4 threads for software x264 on modern CPUs to reduce
        // encode backlog and dropped frames at higher resolutions.
        let encoding_threads = std::thread::available_parallelism()
            .map(|n| n.get().saturating_sub(1).max(1))
            .unwrap_or(2)
            .to_string();
        let selected_encoder = self.settings.video_encoder.resolve(&self.video_encoders);

        info!(
            "Resolved encoder: {:?} (from {:?})",
            selected_encoder, self.settings.video_encoder
        );

        let has_audio = self.settings.capture_audio && self.settings.audio_device.is_some();

        // Pre-compute base arg count to avoid reallocs:
        // ~4 global flags + ~12 for video input + ~6 for audio + ~16 for
        // encoder + ~10 for segment muxer + ~4 vf.
        let mut base_args: Vec<String> = Vec::with_capacity(56);

        // Keep FFmpeg stderr quiet during normal recording.
        // Without this, default `-stats` progress output can fill the stderr
        // pipe over long sessions and stall the recorder thread.
        base_args.extend([
            "-hide_banner".into(),
            "-nostats".into(),
            "-loglevel".into(),
            "warning".into(),
        ]);

        // Video input: prefer DDA (Desktop Duplication API) when available to reduce
        // CPU/GDI overhead; fall back to gdigrab for older FFmpeg builds.
        match self.screen_capture_input {
            ScreenCaptureInput::DdaGrab => {
                base_args.extend([
                    "-thread_queue_size".into(),
                    "4096".into(),
                    "-f".into(),
                    "lavfi".into(),
                    "-i".into(),
                    format!("ddagrab=framerate={}", fps),
                ]);
            }
            ScreenCaptureInput::GdiGrab => {
                base_args.extend([
                    "-thread_queue_size".into(),
                    "4096".into(),
                    "-f".into(),
                    "gdigrab".into(),
                    "-framerate".into(),
                    fps.clone(),
                    "-rtbufsize".into(),
                    "1024M".into(),
                    "-i".into(),
                    "desktop".into(),
                ]);
            }
        }

        // Audio input: dshow (if enabled and device available)
        if has_audio {
            let device = self.settings.audio_device.as_ref().unwrap();
            info!("Audio capture enabled — device: {}", device);
            base_args.extend([
                "-thread_queue_size".into(),
                "4096".into(),
                "-f".into(),
                "dshow".into(),
                "-i".into(),
                format!("audio={}", device),
            ]);
        }

        let mut encoders_to_try = vec![selected_encoder];
        if self.settings.video_encoder == VideoEncoder::Auto
            && selected_encoder != VideoEncoder::Libx264
        {
            encoders_to_try.push(VideoEncoder::Libx264);
        }

        let mut last_error: Option<String> = None;
        for encoder in encoders_to_try {
            let mut args = base_args.clone();

            // Build video filter chain — varies by encoder type.
            // ddagrab emits GPU-backed d3d11 frames that must be downloaded
            // to system memory.  Only NVENC can accept nv12 directly from
            // hwdownload; AMF/QSV and software all need bgra.
            let mut vfilters: Vec<String> = Vec::new();
            if self.screen_capture_input == ScreenCaptureInput::DdaGrab {
                vfilters.push("hwdownload".into());
                vfilters.push("format=bgra".into());
            }
            if let Some(scale) = self.settings.active_scale_filter() {
                vfilters.push(scale);
            }
            vfilters.push("format=yuv420p".into());

            if !vfilters.is_empty() {
                args.push("-vf".into());
                args.push(vfilters.join(","));
            }

            append_video_encoder_args(
                &mut args,
                encoder,
                &self.settings,
                &encoding_threads,
                &force_kf,
                &keyint_frames,
            );

            // Audio encoding (if audio is being captured)
            if has_audio {
                args.extend([
                    "-c:a".into(),
                    "aac".into(),
                    "-b:a".into(),
                    format!("{}k", self.settings.audio_bitrate_kbps.clamp(64, 512)),
                ]);
                // Prevent audio/video desync when dshow delivers at uneven rates
                args.extend(["-max_muxing_queue_size".into(), "1024".into()]);
            }

            // Segment muxer — use MPEG-TS format so data is flushed to disk
            // immediately (MP4 buffers in memory until the moov atom is written)
            args.extend([
                "-fps_mode".into(),
                "cfr".into(),
                "-f".into(),
                "segment".into(),
                "-segment_format".into(),
                "mpegts".into(),
                "-segment_time".into(),
                segment_duration.to_string(),
                "-segment_wrap".into(),
                wrap.clone(),
                "-reset_timestamps".into(),
                "1".into(),
                "-y".into(),
            ]);

            args.push(segment_pattern.to_str().unwrap().into());

            info!("Trying encoder {:?} — ffmpeg {}", encoder, args.join(" "));

            let mut child = ffmpeg_command()
                .args(&args)
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|e| {
                    error!("Failed to spawn FFmpeg process: {}", e);
                    format!("Failed to spawn FFmpeg: {}", e)
                })?;

            // Poll for up to 1 second to check if FFmpeg stays alive.
            // Some encoders (e.g. NVENC without CUDA) crash after 500-700ms.
            let mut encoder_failed = false;
            let check_deadline = Instant::now() + Duration::from_secs(1);
            loop {
                std::thread::sleep(Duration::from_millis(200));
                match child.try_wait() {
                    Ok(Some(status)) => {
                        let mut stderr_text = String::new();
                        if let Some(mut stderr) = child.stderr.take() {
                            let _ = stderr.read_to_string(&mut stderr_text);
                        }

                        error!(
                            "FFmpeg exited early with {} using encoder {:?}",
                            status, encoder,
                        );
                        if !stderr_text.is_empty() {
                            error!("FFmpeg stderr output:\n{}", stderr_text);
                        }

                        let short_err = stderr_text
                            .lines()
                            .rev()
                            .find(|line| !line.trim().is_empty())
                            .unwrap_or("Unknown FFmpeg startup error.")
                            .trim()
                            .to_string();
                        last_error = Some(format!(
                            "FFmpeg exited while starting {} (status: {}). {}",
                            encoder.label(),
                            status,
                            short_err
                        ));
                        encoder_failed = true;
                        break;
                    }
                    Ok(None) => {
                        if Instant::now() >= check_deadline {
                            break; // Still running after 2s — encoder is working
                        }
                    }
                    Err(e) => {
                        error!("Failed to query FFmpeg process state: {}", e);
                        last_error = Some(format!("Failed to query FFmpeg process state: {}", e));
                        encoder_failed = true;
                        break;
                    }
                }
            }

            if encoder_failed {
                continue; // Try next encoder
            }

            // FFmpeg is still running — drain stderr in background with buffered I/O
            if let Some(stderr) = child.stderr.take() {
                std::thread::spawn(move || {
                    let reader = BufReader::with_capacity(4096, stderr);
                    for line in reader.lines() {
                        match line {
                            Ok(text) => {
                                let trimmed = text.trim();
                                if !trimmed.is_empty() {
                                    debug!("[ffmpeg] {}", trimmed);
                                }
                            }
                            Err(_) => break,
                        }
                    }
                });
            }
            info!("Recording started successfully with encoder {:?}", encoder);
            self.child = Some(child);
            self.temp_dir = Some(temp_dir);
            self.started_at = Some(Instant::now());
            self.state = RecorderState::Recording;
            self.restart_attempts = 0;
            self.next_restart_attempt_at = None;
            return Ok(());
        }

        let err = last_error.unwrap_or_else(|| {
            "FFmpeg failed to start recording. Try Software (x264) encoder.".into()
        });
        error!("All encoder attempts failed: {}", err);
        self.state = RecorderState::Idle;
        Err(err)
    }

    /// Stop recording and terminate the FFmpeg process.
    ///
    /// This sends a 'q' signal to FFmpeg for a graceful shutdown, ensuring
    /// the last segment is properly closed.
    pub fn stop(&mut self) {
        info!("Stopping recording (elapsed: {}s)", self.elapsed_seconds());
        if let Some(mut child) = self.child.take() {
            graceful_shutdown(&mut child);
        }
        self.state = RecorderState::Idle;
        self.started_at = None;
        self.temp_dir = None;
        self.recording_expected = false;
        self.restart_attempts = 0;
        self.next_restart_attempt_at = None;
    }

    /// Save a clip without holding the shared recorder mutex during the expensive
    /// concat phase. This keeps the app responsive during long-running saves.
    pub fn save_clip_auto_detached(recorder: &Arc<Mutex<Recorder>>) -> Result<PathBuf, String> {
        let job = {
            let mut rec = recorder
                .lock()
                .map_err(|_| "Recorder lock poisoned while preparing save.".to_string())?;
            let output_path = rec.auto_output_path();
            info!("Auto-saving clip to: {}", output_path.display());
            rec.prepare_save_job(output_path)?
        };

        let execution = execute_save_job(job);

        let mut rec = recorder
            .lock()
            .map_err(|_| "Recorder lock poisoned while finalizing save.".to_string())?;
        rec.finish_save_job(execution)
    }

    /// Auto-save the current replay buffer to the configured output directory.
    #[allow(dead_code)]
    pub fn save_clip_auto(&mut self) -> Result<PathBuf, String> {
        let output_path = self.auto_output_path();
        info!("Auto-saving clip to: {}", output_path.display());
        self.save_clip(&output_path)
    }

    /// Save the current replay buffer to a specific path.
    #[allow(dead_code)]
    pub fn save_clip(&mut self, output_path: &Path) -> Result<PathBuf, String> {
        let save_job = self.prepare_save_job(output_path.to_path_buf())?;
        let execution = execute_save_job(save_job);
        self.finish_save_job(execution)
    }

    /// Called periodically by a watchdog thread to detect FFmpeg exits and
    /// attempt bounded automatic recovery when recording was expected.
    pub fn health_check_tick(&mut self) {
        if matches!(self.state, RecorderState::Recording | RecorderState::Saving) {
            let stopped_reason = match self.child.as_mut() {
                Some(child) => match child.try_wait() {
                    Ok(Some(status)) => Some(format!("FFmpeg exited with status {}", status)),
                    Ok(None) => {
                        self.restart_attempts = 0;
                        self.next_restart_attempt_at = None;
                        return;
                    }
                    Err(e) => Some(format!("Failed to poll FFmpeg process: {}", e)),
                },
                None => Some("Missing FFmpeg child while state is Recording".into()),
            };

            if let Some(reason) = stopped_reason {
                self.mark_unexpected_stop(&reason);
            }
        }

        if self.state == RecorderState::Idle && self.recording_expected {
            let now = Instant::now();
            let restart_due = self
                .next_restart_attempt_at
                .map(|scheduled| now >= scheduled)
                .unwrap_or(true);
            if !restart_due {
                return;
            }

            let attempt_number = self.restart_attempts.saturating_add(1);
            warn!(
                "Attempting recorder auto-restart (attempt #{})",
                attempt_number
            );

            match self.start() {
                Ok(()) => {
                    info!("Recorder auto-restart succeeded");
                    self.restart_attempts = 0;
                    self.next_restart_attempt_at = None;
                }
                Err(e) => {
                    if !self.recording_expected {
                        return;
                    }
                    self.state = RecorderState::Idle;
                    self.restart_attempts = self.restart_attempts.saturating_add(1);
                    let delay = Self::restart_backoff_delay(self.restart_attempts);
                    self.next_restart_attempt_at = Some(Instant::now() + delay);
                    error!(
                        "Recorder auto-restart failed: {}. Next retry in {}s",
                        e,
                        delay.as_secs()
                    );
                }
            }
        }
    }

    /// Generate an auto-save output path with timestamp.
    fn auto_output_path(&self) -> PathBuf {
        let now = chrono::Local::now();
        let filename = format!("LiteClipReplay_{}.mp4", now.format("%Y-%m-%d_%H-%M-%S"));
        self.settings.output_dir.join(filename)
    }

    fn prepare_save_job(&mut self, output_path: PathBuf) -> Result<SaveJob, String> {
        if self.state == RecorderState::Saving {
            warn!("Save requested while another save is in progress");
            return Err("Save already in progress.".into());
        }

        let resume_state = self.state;
        self.state = RecorderState::Saving;
        info!(
            "Save clip requested — resume_state={:?}, elapsed={}s, output={}",
            resume_state,
            self.elapsed_seconds(),
            output_path.display(),
        );

        let segment_root = self
            .temp_dir
            .as_ref()
            .ok_or_else(|| "No recording buffer available yet.".to_string())?
            .path()
            .to_path_buf();
        let segment_paths = collect_segments_for_snapshot(&segment_root, self.child.is_some())?;

        if segment_paths.is_empty() {
            return Err(
                "Replay buffer is still warming up. Wait a few seconds, then try save again."
                    .into(),
            );
        }

        Ok(SaveJob {
            resume_state,
            output_path,
            segment_paths,
        })
    }

    fn finish_save_job(&mut self, execution: SaveExecution) -> Result<PathBuf, String> {
        self.state = if execution.resume_state == RecorderState::Recording
            && self.recording_expected
            && self.child.is_some()
        {
            RecorderState::Recording
        } else {
            RecorderState::Idle
        };

        if let Ok(saved_path) = &execution.save_result {
            self.last_saved_path = Some(saved_path.clone());
        }

        execution.save_result
    }

    fn mark_unexpected_stop(&mut self, reason: &str) {
        error!("Recorder stopped unexpectedly: {}", reason);
        self.child = None;
        self.temp_dir = None;
        self.started_at = None;
        self.state = RecorderState::Idle;

        if self.recording_expected && self.next_restart_attempt_at.is_none() {
            self.next_restart_attempt_at = Some(Instant::now());
        }
    }

    fn restart_backoff_delay(attempts: u32) -> Duration {
        let shift = attempts.saturating_sub(1).min(5);
        let secs = 1u64 << shift;
        Duration::from_secs(secs.min(30))
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        info!("Recorder shutting down");
        self.stop();
    }
}

// ── Helpers ────────────────────────────────────────────────────────

fn execute_save_job(save_job: SaveJob) -> SaveExecution {
    let output_path = save_job.output_path.clone();
    let save_result = (|| -> Result<PathBuf, String> {
        info!(
            "Saving snapshot with {} segment(s)",
            save_job.segment_paths.len()
        );

        let snapshot_dir = TempDir::new().map_err(|e| format!("Failed to create snapshot dir: {e}"))?;
        let copied_segments = copy_segments_into_snapshot(&save_job.segment_paths, snapshot_dir.path())?;

        let concat_list_path = snapshot_dir.path().join("concat_list.txt");
        let mut concat_file = fs::File::create(&concat_list_path)
            .map_err(|e| format!("Failed to create concat list: {e}"))?;
        for seg in &copied_segments {
            let escaped_path = escape_ffmpeg_concat_path(seg);
            writeln!(concat_file, "file '{}'", escaped_path)
                .map_err(|e| format!("Failed to write concat list: {e}"))?;
        }
        drop(concat_file);

        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create output directory: {e}"))?;
        }

        let concat_args = [
            "-nostdin",
            "-probesize",
            "32",
            "-analyzeduration",
            "0",
            "-f",
            "concat",
            "-safe",
            "0",
            "-i",
            concat_list_path.to_str().unwrap_or_default(),
            "-c",
            "copy",
            "-y",
            output_path.to_str().unwrap_or_default(),
        ];
        info!("Running concat: ffmpeg {}", concat_args.join(" "));

        let concat_result = ffmpeg_command()
            .args(concat_args)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .map_err(|e| format!("Failed to run FFmpeg concat: {e}"))?;

        if !concat_result.status.success() {
            let stderr = String::from_utf8_lossy(&concat_result.stderr);
            if !stderr.is_empty() {
                error!("FFmpeg concat stderr:\n{}", stderr);
            }
            return Err("FFmpeg concat failed. Check segment files.".into());
        }

        let file_size = fs::metadata(&output_path).map(|m| m.len()).unwrap_or(0);
        info!(
            "Clip saved successfully: {} ({:.1} MB)",
            output_path.display(),
            file_size as f64 / (1024.0 * 1024.0),
        );

        Ok(output_path.clone())
    })();

    SaveExecution {
        resume_state: save_job.resume_state,
        save_result,
    }
}

/// Gracefully shutdown an FFmpeg child process.
/// Sends 'q' to stdin, waits up to 2 seconds, then force-kills if needed.
fn graceful_shutdown(child: &mut Child) {
    // Try sending 'q' + newline for graceful shutdown
    if let Some(ref mut stdin) = child.stdin {
        let _ = stdin.write_all(b"q\n");
        let _ = stdin.flush();
    }
    // Drop stdin to signal EOF — helps FFmpeg realize it should quit
    child.stdin.take();

    // Wait up to 2 seconds for FFmpeg to exit on its own (poll at 50ms)
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                info!("FFmpeg exited: {}", status);
                return;
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    warn!("FFmpeg did not exit within 2s — force-killing");
                    let _ = child.kill();
                    let _ = child.wait();
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => {
                error!("Error checking FFmpeg status: {}", e);
                let _ = child.kill();
                let _ = child.wait();
                return;
            }
        }
    }
}

fn collect_segments_for_snapshot(
    segment_root: &Path,
    recording_active: bool,
) -> Result<Vec<PathBuf>, String> {
    let mut segments: Vec<PathBuf> = fs::read_dir(segment_root)
        .map_err(|e| format!("Failed to read temp dir {}: {e}", segment_root.display()))?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("ts") {
                Some(path)
            } else {
                None
            }
        })
        .collect();

    if segments.is_empty() {
        return Ok(Vec::new());
    }

    segments.sort_unstable_by_key(|path| extract_segment_index(path));
    if segments.len() > 1 {
        reorder_wrapped_segments(segments.as_mut_slice());
    }

    // While recording is active, the newest segment can still be in-progress.
    // Dropping it greatly improves concat reliability.
    if recording_active && segments.len() > 1 {
        segments.pop();
    }

    segments.retain(|path| is_stable_segment(path));

    Ok(segments)
}

fn is_stable_segment(path: &Path) -> bool {
    let Ok(meta) = fs::metadata(path) else {
        return false;
    };
    if meta.len() < MIN_STABLE_SEGMENT_BYTES {
        return false;
    }
    let Ok(modified) = meta.modified() else {
        return false;
    };
    let Ok(age) = modified.elapsed() else {
        return false;
    };
    age.as_millis() >= STABLE_SEGMENT_GUARD_MS
}

fn copy_segments_into_snapshot(
    segment_paths: &[PathBuf],
    snapshot_root: &Path,
) -> Result<Vec<PathBuf>, String> {
    let mut copied = Vec::with_capacity(segment_paths.len());
    for (index, source) in segment_paths.iter().enumerate() {
        let target = snapshot_root.join(format!("seg_{index:04}.ts"));
        fs::copy(source, &target)
            .map_err(|e| format!("Failed to snapshot segment {}: {e}", source.display()))?;
        copied.push(target);
    }

    if copied.is_empty() {
        return Err("No stable segments available yet. Try again in a moment.".into());
    }

    Ok(copied)
}

/// Extract the numeric index from a segment filename like `seg_0003.ts`.
fn extract_segment_index(path: &Path) -> u32 {
    path.file_stem()
        .and_then(|s| s.to_str())
        .and_then(|s| s.strip_prefix("seg_"))
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(u32::MAX)
}

/// Escape a path for FFmpeg concat demuxer list files.
fn escape_ffmpeg_concat_path(path: &Path) -> String {
    // FFmpeg concat demuxer is sensitive to Windows backslashes because they
    // are interpreted as escapes in the list format. Normalize to forward
    // slashes first, then escape single quotes for the surrounding quotes.
    path.to_string_lossy()
        .replace('\\', "/")
        .replace('\'', "'\\''")
}

/// Reorder wrapped segments so the oldest segment is first.
/// When segment_wrap is active, FFmpeg overwrites old segments in a ring.
/// The newest segment has the highest mtime; the segment right after it
/// (by index) is the oldest one still on disk.
fn reorder_wrapped_segments(segments: &mut [PathBuf]) {
    // Find the position where mtime jumps backwards (wrap point).
    let mtimes: Vec<_> = segments
        .iter()
        .map(|p| {
            fs::metadata(p)
                .and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
        })
        .collect();

    // Look for the newest segment (highest mtime). Everything after it is older.
    let mut newest_idx = 0;
    for i in 1..mtimes.len() {
        if mtimes[i] > mtimes[newest_idx] {
            newest_idx = i;
        }
    }

    // If the newest is not the last element, segments have wrapped.
    // Rotate so the segment after the newest is first.
    if newest_idx + 1 < segments.len() {
        segments.rotate_left(newest_idx + 1);
    }
}

/// Detect available audio recording devices via FFmpeg.
fn detect_audio_devices() -> Vec<String> {
    debug!("Detecting audio devices via FFmpeg dshow...");
    let output = ffmpeg_command()
        .args(["-list_devices", "true", "-f", "dshow", "-i", "dummy"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output();

    let output = match output {
        Ok(o) => o,
        Err(e) => {
            warn!("Failed to run FFmpeg for device detection: {}", e);
            return Vec::new();
        }
    };

    // FFmpeg lists devices on stderr.
    // Lines look like:  [dshow @ 0x...] "Microphone (Realtek ...)" (audio)
    // Alternative names: [dshow @ 0x...]   Alternative name "@device_cm_..."
    let stderr = String::from_utf8_lossy(&output.stderr);
    debug!("FFmpeg device list output:\n{}", stderr);
    let mut devices = Vec::new();

    for line in stderr.lines() {
        // Only care about lines tagged as "(audio)"
        if !line.contains("(audio)") {
            continue;
        }
        // Extract the quoted device name
        if let Some(start) = line.find('"') {
            if let Some(end) = line[start + 1..].find('"') {
                let name = &line[start + 1..start + 1 + end];
                // Skip alternative-name lines (start with @)
                if !name.starts_with('@') {
                    devices.push(name.to_string());
                }
            }
        }
    }

    info!("Detected {} audio device(s): {:?}", devices.len(), devices);
    devices
}

/// Get a unique string identifying your FFmpeg binary (e.g. version output).
fn get_ffmpeg_version_string() -> Option<String> {
    let output = ffmpeg_command()
        .arg("-version")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;

    if output.status.success() {
        String::from_utf8(output.stdout).ok()
    } else {
        None
    }
}

fn load_recorder_cache(cache_path: &Path, current_version: &str) -> Option<RecorderCache> {
    let file = fs::File::open(cache_path).ok()?;
    let reader = BufReader::new(file);
    let cache: RecorderCache = serde_json::from_reader(reader).ok()?;

    if cache.ffmpeg_version_string == current_version {
        Some(cache)
    } else {
        debug!("Recorder cache invalid: FFmpeg version mismatch");
        None
    }
}

fn save_recorder_cache(cache_path: &Path, cache: RecorderCache) {
    // Ensure parent dir exists
    if let Some(parent) = cache_path.parent() {
        let _ = fs::create_dir_all(parent);
    }

    if let Ok(file) = fs::File::create(cache_path) {
        let writer = BufWriter::new(file);
        if let Err(e) = serde_json::to_writer(writer, &cache) {
            warn!("Failed to write recorder cache: {}", e);
        } else {
            debug!("Recorder cache saved to {}", cache_path.display());
        }
    }
}

fn detect_video_encoders(ffmpeg_found: bool) -> Vec<VideoEncoder> {
    let mut encoders = vec![VideoEncoder::Libx264];
    if !ffmpeg_found {
        return encoders;
    }

    // Note: Caching logic moved to Recorder::new()

    debug!("Detecting available hardware video encoders...");
    let output = ffmpeg_command()
        .args(["-hide_banner", "-encoders"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output();

    let output = match output {
        Ok(o) => o,
        Err(e) => {
            warn!("Failed to query FFmpeg encoders: {}", e);
            return encoders;
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout);

    // For each hardware encoder, check if it's compiled in AND actually works
    // by running a quick test encode. This catches cases like NVENC being
    // compiled into FFmpeg but nvcuda.dll not being available.
    let hw_candidates = [
        (" h264_nvenc", VideoEncoder::H264Nvenc, "NVIDIA NVENC"),
        (" h264_qsv", VideoEncoder::H264Qsv, "Intel Quick Sync"),
        (" h264_amf", VideoEncoder::H264Amf, "AMD AMF"),
    ];

    for (search_str, encoder_variant, label) in hw_candidates {
        if !stdout.contains(search_str) {
            continue;
        }
        let ffmpeg_name = encoder_variant.ffmpeg_name().unwrap_or_default();
        debug!("Testing hardware encoder {} ({})", label, ffmpeg_name);

        // Quick 1-frame probe: encode a tiny synthetic nv12 frame.
        let probe = ffmpeg_command()
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=c=black:s=256x256:d=0.04,format=nv12",
                "-frames:v",
                "1",
                "-c:v",
                ffmpeg_name,
                "-f",
                "null",
                "-",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output();

        match probe {
            Ok(result) if result.status.success() => {
                info!("Hardware encoder verified: {}", label);
                encoders.push(encoder_variant);
            }
            Ok(result) => {
                let stderr = String::from_utf8_lossy(&result.stderr);
                let reason = stderr
                    .lines()
                    .find(|l| l.contains("Cannot load") || l.contains("Error"))
                    .unwrap_or("probe encode failed")
                    .trim();
                warn!(
                    "Hardware encoder {} compiled in but not usable: {}",
                    label, reason
                );
            }
            Err(e) => {
                warn!("Failed to probe encoder {}: {}", label, e);
            }
        }
    }

    encoders
}

fn detect_screen_capture_input(ffmpeg_found: bool) -> ScreenCaptureInput {
    if !ffmpeg_found {
        return ScreenCaptureInput::GdiGrab;
    }

    debug!("Detecting FFmpeg screen capture filters...");
    let output = ffmpeg_command()
        .args(["-hide_banner", "-filters"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output();

    let output = match output {
        Ok(o) => o,
        Err(e) => {
            warn!("Failed to query FFmpeg filters: {}", e);
            return ScreenCaptureInput::GdiGrab;
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    if stdout.contains(" ddagrab ") {
        info!("FFmpeg filter ddagrab detected");
        ScreenCaptureInput::DdaGrab
    } else {
        info!("FFmpeg filter ddagrab not found; using gdigrab");
        ScreenCaptureInput::GdiGrab
    }
}

/// Append encoder-specific FFmpeg arguments.
///
/// Uses native preset names for each encoder instead of assuming x264 semantics.
fn append_video_encoder_args(
    args: &mut Vec<String>,
    encoder: VideoEncoder,
    settings: &Settings,
    encoding_threads: &str,
    force_kf: &str,
    keyint_frames: &str,
) {
    let quality = settings.quality;
    let use_advanced = settings.advanced_video_controls;
    let rate_control = settings.rate_control;
    let video_bitrate = settings.video_bitrate_kbps.clamp(1000, 150000);
    let max_bitrate = settings.video_max_bitrate_kbps.clamp(video_bitrate, 200000);
    let bufsize = settings.video_bufsize_kbps.clamp(video_bitrate, 400000);
    let crf = settings.video_crf.clamp(0, 51);

    match encoder {
        VideoEncoder::Libx264 => {
            let preset = if use_advanced {
                settings.encoder_tuning.x264_preset()
            } else {
                quality.preset()
            };

            args.extend(["-c:v".into(), "libx264".into()]);
            args.extend(["-threads".into(), encoding_threads.to_string()]);
            args.extend(["-preset".into(), preset.into()]);

            match if use_advanced {
                rate_control
            } else {
                RateControl::Preset
            } {
                RateControl::Preset | RateControl::Crf => {
                    let effective_crf = if use_advanced { crf } else { quality.crf() };
                    args.extend(["-crf".into(), effective_crf.to_string()]);
                }
                RateControl::Cbr => {
                    args.extend(["-b:v".into(), format!("{}k", video_bitrate)]);
                    args.extend(["-maxrate".into(), format!("{}k", video_bitrate)]);
                    args.extend(["-bufsize".into(), format!("{}k", bufsize)]);
                }
                RateControl::Vbr => {
                    args.extend(["-b:v".into(), format!("{}k", video_bitrate)]);
                    args.extend(["-maxrate".into(), format!("{}k", max_bitrate)]);
                    args.extend(["-bufsize".into(), format!("{}k", bufsize)]);
                }
            }

            args.extend(["-g".into(), keyint_frames.into()]);
            args.extend(["-keyint_min".into(), "1".into()]);
            args.extend(["-pix_fmt".into(), "yuv420p".into()]);
            args.extend(["-force_key_frames".into(), force_kf.into()]);
        }
        VideoEncoder::H264Nvenc => {
            let (target_kbps, max_kbps, buf_kbps) = if use_advanced {
                (video_bitrate, max_bitrate, bufsize)
            } else {
                let base = quality.target_bitrate_kbps();
                (base, base * 2, base * 2)
            };
            let preset = if use_advanced {
                settings.encoder_tuning.nvenc_preset()
            } else {
                quality.nvenc_preset()
            };

            args.extend([
                "-c:v".into(),
                "h264_nvenc".into(),
                "-preset".into(),
                preset.into(),
                "-b:v".into(),
                format!("{}k", target_kbps),
                "-maxrate".into(),
                format!("{}k", max_kbps),
                "-bufsize".into(),
                format!("{}k", buf_kbps),
                "-g".into(),
                keyint_frames.into(),
                "-pix_fmt".into(),
                "yuv420p".into(),
            ]);
        }
        VideoEncoder::H264Qsv => {
            let (target_kbps, max_kbps, buf_kbps) = if use_advanced {
                (video_bitrate, max_bitrate, bufsize)
            } else {
                let base = quality.target_bitrate_kbps();
                (base, base * 2, base * 2)
            };
            let preset = if use_advanced {
                settings.encoder_tuning.qsv_preset()
            } else {
                quality.qsv_preset()
            };

            args.extend([
                "-c:v".into(),
                "h264_qsv".into(),
                "-preset".into(),
                preset.into(),
                "-b:v".into(),
                format!("{}k", target_kbps),
                "-maxrate".into(),
                format!("{}k", max_kbps),
                "-bufsize".into(),
                format!("{}k", buf_kbps),
                "-g".into(),
                keyint_frames.into(),
                "-pix_fmt".into(),
                "yuv420p".into(),
            ]);
        }
        VideoEncoder::H264Amf => {
            let (target_kbps, max_kbps, buf_kbps) = if use_advanced {
                (video_bitrate, max_bitrate, bufsize)
            } else {
                let base = quality.target_bitrate_kbps();
                (base, base * 2, base * 2)
            };
            let amf_quality = if use_advanced {
                settings.encoder_tuning.amf_quality()
            } else {
                quality.amf_quality()
            };

            args.extend([
                "-c:v".into(),
                "h264_amf".into(),
                "-quality".into(),
                amf_quality.into(),
                "-b:v".into(),
                format!("{}k", target_kbps),
                "-maxrate".into(),
                format!("{}k", max_kbps),
                "-bufsize".into(),
                format!("{}k", buf_kbps),
                "-g".into(),
                keyint_frames.into(),
                "-pix_fmt".into(),
                "yuv420p".into(),
            ]);
        }
        VideoEncoder::Auto => {} // resolved before reaching here
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recorder_for_tests() -> Recorder {
        Recorder {
            state: RecorderState::Idle,
            settings: Settings::default(),
            ffmpeg_found: false,
            last_saved_path: None,
            audio_devices: Vec::new(),
            video_encoders: vec![VideoEncoder::Libx264],
            screen_capture_input: ScreenCaptureInput::GdiGrab,
            child: None,
            temp_dir: None,
            started_at: None,
            recording_expected: false,
            restart_attempts: 0,
            next_restart_attempt_at: None,
        }
    }

    #[test]
    fn start_without_ffmpeg_fails_cleanly() {
        let mut recorder = recorder_for_tests();
        let err = recorder
            .start()
            .expect_err("start should fail without ffmpeg");
        assert!(err.contains("FFmpeg not found"));
        assert_eq!(recorder.state, RecorderState::Idle);
        assert!(!recorder.recording_expected);
    }

    #[test]
    fn stop_resets_recovery_state() {
        let mut recorder = recorder_for_tests();
        recorder.state = RecorderState::Recording;
        recorder.recording_expected = true;
        recorder.next_restart_attempt_at = Some(Instant::now() + Duration::from_secs(10));
        recorder.restart_attempts = 3;

        recorder.stop();

        assert_eq!(recorder.state, RecorderState::Idle);
        assert!(!recorder.recording_expected);
        assert!(recorder.next_restart_attempt_at.is_none());
        assert_eq!(recorder.restart_attempts, 0);
    }

    #[test]
    fn save_finalize_updates_last_saved_path_without_restart() {
        let mut recorder = recorder_for_tests();
        recorder.state = RecorderState::Recording;
        let temp_dir = TempDir::new().expect("temp dir");
        let seg0 = temp_dir.path().join("seg_0000.ts");
        let seg1 = temp_dir.path().join("seg_0001.ts");
        fs::write(&seg0, vec![0x47u8; (MIN_STABLE_SEGMENT_BYTES as usize) + 1024])
            .expect("write seg0");
        std::thread::sleep(Duration::from_millis((STABLE_SEGMENT_GUARD_MS as u64) + 20));
        fs::write(&seg1, vec![0x47u8; (MIN_STABLE_SEGMENT_BYTES as usize) + 2048])
            .expect("write seg1");
        recorder.temp_dir = Some(temp_dir);
        recorder.recording_expected = false;
        let output_path = PathBuf::from(r"C:\clips\test_clip.mp4");

        let save_job = recorder
            .prepare_save_job(output_path.clone())
            .expect("prepare save should succeed");
        assert_eq!(recorder.state, RecorderState::Saving);

        let execution = SaveExecution {
            resume_state: save_job.resume_state,
            save_result: Ok(output_path.clone()),
        };

        let saved = recorder
            .finish_save_job(execution)
            .expect("finalize save should succeed");
        assert_eq!(saved, output_path);
        assert_eq!(recorder.state, RecorderState::Idle);
        assert_eq!(recorder.last_saved_path.as_ref(), Some(&saved));
    }

    #[cfg(windows)]
    #[test]
    fn health_check_detects_dead_child_and_transitions_to_idle() {
        let mut recorder = recorder_for_tests();
        recorder.ffmpeg_found = true;
        recorder.state = RecorderState::Recording;
        recorder.recording_expected = true;
        recorder.next_restart_attempt_at = Some(Instant::now() + Duration::from_secs(60));
        let mut child = Command::new("cmd")
            .args(["/C", "exit", "0"])
            .stdin(Stdio::piped())
            .spawn()
            .expect("spawn short-lived child");
        for _ in 0..100 {
            if child
                .try_wait()
                .expect("polling child process should not fail")
                .is_some()
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        recorder.child = Some(child);

        recorder.health_check_tick();

        assert_eq!(recorder.state, RecorderState::Idle);
        assert!(recorder.child.is_none());
        assert!(recorder.recording_expected);
    }

    #[test]
    fn restart_backoff_is_capped() {
        assert_eq!(Recorder::restart_backoff_delay(1), Duration::from_secs(1));
        assert_eq!(Recorder::restart_backoff_delay(2), Duration::from_secs(2));
        assert_eq!(Recorder::restart_backoff_delay(3), Duration::from_secs(4));
        assert_eq!(Recorder::restart_backoff_delay(8), Duration::from_secs(30));
    }

    #[test]
    fn concat_escape_normalizes_backslashes() {
        let input = PathBuf::from(r"C:\Temp\clip\seg_0001.ts");
        let escaped = escape_ffmpeg_concat_path(&input);
        assert!(escaped.contains("C:/Temp/clip/seg_0001.ts"));
        assert!(!escaped.contains(r"\"));
    }
}
