use log::{debug, error, info, warn};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use tempfile::TempDir;

use crate::settings::{Quality, Settings, VideoEncoder};

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
    let mut cmd = Command::new("ffmpeg");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Manages the FFmpeg subprocess and segment-based replay buffer.
///
/// The recorder uses FFmpeg to capture screen and audio into 10-second rolling segments.
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
}

impl Recorder {
    /// Create a new Recorder with default settings.
    pub fn new() -> Self {
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
            .join("LiteClip");
        let cache_path = cache_dir.join("recorder_cache.json");
        let ffmpeg_version = get_ffmpeg_version_string().unwrap_or_default();

        let (audio_devices, video_encoders) = if let Some(cache) = load_recorder_cache(&cache_path, &ffmpeg_version) {
            info!("Loaded capabilities from cache: {} audio devices, {} encoders", 
                cache.audio_devices.len(), cache.video_encoders.len());
            (cache.audio_devices, cache.video_encoders)
        } else {
            info!("Capabilities cache missing or invalid — probing system...");
            let audio_devices = detect_audio_devices();
            let video_encoders = detect_video_encoders(ffmpeg_found);
            
            if !ffmpeg_version.is_empty() {
                save_recorder_cache(&cache_path, RecorderCache {
                    ffmpeg_version_string: ffmpeg_version,
                    video_encoders: video_encoders.clone(),
                    audio_devices: audio_devices.clone(),
                });
            }
            (audio_devices, video_encoders)
        };

        info!("Final Audio Devices: {:?}", audio_devices);
        info!("Final Video Encoders: {:?}", video_encoders);
        
        let screen_capture_input = detect_screen_capture_input(ffmpeg_found);
        info!(
            "Selected screen capture input: {:?}",
            screen_capture_input
        );

        let mut settings = Settings::load();

        // If no audio device is saved, or the saved one is not in the detected list,
        // fallback to the first available device.
        if !audio_devices.is_empty() {
             let device_valid = settings.audio_device.as_ref()
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
        }
    }

    /// How many segment files to keep (buffer / 10s segments).
    fn segment_wrap(&self) -> u64 {
        (self.settings.buffer_seconds / 10).max(1)
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
            return Err("FFmpeg not found on PATH. Please install FFmpeg.".into());
        }
        if self.state == RecorderState::Recording {
            warn!("Start called but already recording — ignoring");
            return Err("Already recording.".into());
        }

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

        let segment_duration: u64 = 10;
        let force_kf = format!("expr:gte(t,n_forced*{})", segment_duration);
        let fps = self.settings.framerate.value().to_string();
        let crf = self.settings.quality.crf().to_string();
        let wrap = self.segment_wrap().to_string();
        // Keep at least one logical core free so desktop input/rendering stays responsive.
        let encoding_threads = std::thread::available_parallelism()
            .map(|n| n.get().saturating_sub(1).clamp(1, 4))
            .unwrap_or(2)
            .to_string();
        let selected_encoder = self.settings.video_encoder.resolve(&self.video_encoders);

        info!(
            "Resolved encoder: {:?} (from {:?})",
            selected_encoder, self.settings.video_encoder
        );

        let has_audio =
            self.settings.capture_audio && self.settings.audio_device.is_some();

        // Pre-compute base arg count to avoid reallocs:
        // ~12 for video input + ~4 for audio + ~16 for encoder + ~10 for segment muxer + ~4 vf
        let mut base_args: Vec<String> = Vec::with_capacity(48);

        // Video input: prefer DDA (Desktop Duplication API) when available to reduce
        // CPU/GDI overhead; fall back to gdigrab for older FFmpeg builds.
        match self.screen_capture_input {
            ScreenCaptureInput::DdaGrab => {
                base_args.extend([
                    "-f".into(),
                    "lavfi".into(),
                    "-i".into(),
                    format!("ddagrab=framerate={}", fps),
                ]);
            }
            ScreenCaptureInput::GdiGrab => {
                base_args.extend([
                    "-f".into(),
                    "gdigrab".into(),
                    "-framerate".into(),
                    fps.clone(),
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

            let is_hw = encoder.is_hardware();

            // Build video filter chain — varies by encoder type.
            // ddagrab emits GPU-backed d3d11 frames that must be downloaded
            // to system memory.  Only NVENC can accept nv12 directly from
            // hwdownload; AMF/QSV and software all need bgra.
            let mut vfilters = Vec::new();
            if self.screen_capture_input == ScreenCaptureInput::DdaGrab {
                vfilters.push("hwdownload");
                if encoder == VideoEncoder::H264Nvenc {
                    // NVENC: skip bgra intermediate — go directly to nv12
                    vfilters.push("format=nv12");
                } else {
                    vfilters.push("format=bgra");
                }
            }
            if let Some(scale) = self.settings.resolution.scale_filter() {
                vfilters.push(scale);
            }
            if !vfilters.is_empty() {
                args.push("-vf".into());
                args.push(vfilters.join(","));
            }

            append_video_encoder_args(
                &mut args,
                encoder,
                &self.settings.quality,
                &encoding_threads,
                &crf,
                &force_kf,
            );

            // Audio encoding (if audio is being captured)
            if has_audio {
                args.extend(["-c:a".into(), "aac".into(), "-b:a".into(), "128k".into()]);
                // Prevent audio/video desync when dshow delivers at uneven rates
                args.extend(["-max_muxing_queue_size".into(), "1024".into()]);
            }

            // Segment muxer — use MPEG-TS format so data is flushed to disk
            // immediately (MP4 buffers in memory until the moov atom is written)
            args.extend([
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
            return Ok(());
        }

        let err = last_error.unwrap_or_else(|| {
            "FFmpeg failed to start recording. Try Software (x264) encoder.".into()
        });
        error!("All encoder attempts failed: {}", err);
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
    }

    /// Auto-save the current replay buffer to the configured output directory.
    pub fn save_clip_auto(&mut self) -> Result<PathBuf, String> {
        let output_path = self.auto_output_path();
        info!("Auto-saving clip to: {}", output_path.display());
        self.save_clip(&output_path)
    }

    /// Save the current replay buffer to a specific path.
    pub fn save_clip(&mut self, output_path: &Path) -> Result<PathBuf, String> {
        let was_recording = self.state == RecorderState::Recording;
        self.state = RecorderState::Saving;
        info!(
            "Save clip requested — was_recording={}, elapsed={}s, output={}",
            was_recording,
            self.elapsed_seconds(),
            output_path.display(),
        );

        // Stop FFmpeg so all segments are flushed
        if let Some(mut child) = self.child.take() {
            info!("Stopping FFmpeg for segment flush...");
            graceful_shutdown(&mut child);
        }

        // Poll briefly for the last segment to be fully written, rather than
        // a fixed 200ms sleep. On fast machines this returns in <50ms.
        wait_for_segments_flush();

        let temp_path = self
            .temp_dir
            .as_ref()
            .ok_or_else(|| {
                error!("Save failed: No temp directory — nothing recorded yet");
                "No temp directory — nothing recorded yet.".to_string()
            })?
            .path()
            .to_path_buf();

        debug!("Scanning temp dir for segments: {}", temp_path.display());

        // Gather segment files sorted by numeric index from filename
        let mut segments: Vec<PathBuf> = fs::read_dir(&temp_path)
            .map_err(|e| {
                error!("Failed to read temp dir {}: {}", temp_path.display(), e);
                format!("Failed to read temp dir: {}", e)
            })?
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
            error!(
                "No segment files found in {}. Buffer may not have recorded long enough.",
                temp_path.display()
            );
            if let Ok(entries) = fs::read_dir(&temp_path) {
                let files: Vec<String> = entries
                    .filter_map(|e| e.ok())
                    .map(|e| format!("{}", e.path().display()))
                    .collect();
                debug!("Temp dir contents: {:?}", files);
            }
            self.restore_state_after_save(was_recording);
            return Err("No segments found. Record for a few seconds first.".into());
        }

        info!("Found {} segment(s) to concat", segments.len());

        // Sort by numeric index extracted from filename (seg_NNNN.ts).
        // This is more reliable than mtime which can alias when segment_wrap
        // reuses filenames — mtime resolution on NTFS is ~100ms and two
        // segments written in rapid succession can get the same timestamp.
        segments.sort_unstable_by(|a, b| {
            extract_segment_index(a).cmp(&extract_segment_index(b))
        });

        // When segment_wrap is active, reorder so oldest segment comes first.
        // The oldest segment has the highest mtime gap from its predecessor.
        if segments.len() > 1 {
            reorder_wrapped_segments(&mut segments);
        }

        for seg in &segments {
            let size = fs::metadata(seg).map(|m| m.len()).unwrap_or(0);
            debug!("  Segment: {} ({} bytes)", seg.display(), size);
        }

        // Create the concat list file
        let concat_list_path = temp_path.join("concat_list.txt");
        let mut concat_file = fs::File::create(&concat_list_path).map_err(|e| {
            error!("Failed to create concat list: {}", e);
            format!("Failed to create concat list: {}", e)
        })?;

        for seg in &segments {
            writeln!(concat_file, "file '{}'", seg.display()).map_err(|e| {
                error!("Failed to write concat list: {}", e);
                format!("Failed to write concat list: {}", e)
            })?;
        }
        drop(concat_file);

        // Ensure output directory exists
        if let Some(parent) = output_path.parent() {
            let _ = fs::create_dir_all(parent);
        }

        // Concatenate segments into final clip.
        // -probesize 32 -analyzeduration 0: segments are known-good MPEG-TS, skip probing.
        // -nostdin: prevent accidental stdin blocking.
        let concat_args = [
            "-nostdin",
            "-probesize", "32",
            "-analyzeduration", "0",
            "-f", "concat",
            "-safe", "0",
            "-i", concat_list_path.to_str().unwrap(),
            "-c", "copy",
            "-y",
            output_path.to_str().unwrap(),
        ];
        info!("Running concat: ffmpeg {}", concat_args.join(" "));

        let concat_result = ffmpeg_command()
            .args(&concat_args)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .map_err(|e| {
                error!("Failed to run FFmpeg concat: {}", e);
                format!("Failed to run FFmpeg concat: {}", e)
            })?;

        if !concat_result.status.success() {
            let stderr = String::from_utf8_lossy(&concat_result.stderr);
            error!("FFmpeg concat failed (status: {})", concat_result.status);
            if !stderr.is_empty() {
                error!("FFmpeg concat stderr:\n{}", stderr);
            }
            self.restore_state_after_save(was_recording);
            return Err("FFmpeg concat failed. Check segment files.".into());
        }

        // Log final file size
        let file_size = fs::metadata(output_path).map(|m| m.len()).unwrap_or(0);
        info!(
            "Clip saved successfully: {} ({:.1} MB)",
            output_path.display(),
            file_size as f64 / (1024.0 * 1024.0),
        );

        self.last_saved_path = Some(output_path.to_path_buf());
        self.restore_state_after_save(was_recording);

        Ok(output_path.to_path_buf())
    }

    /// Generate an auto-save output path with timestamp.
    fn auto_output_path(&self) -> PathBuf {
        let now = chrono::Local::now();
        let filename = format!("LiteClip_{}.mp4", now.format("%Y-%m-%d_%H-%M-%S"));
        self.settings.output_dir.join(filename)
    }

    fn restore_state_after_save(&mut self, was_recording: bool) {
        if was_recording {
            info!("Restarting recording after save...");
            self.temp_dir = None;
            self.started_at = None;
            match self.start() {
                Ok(()) => info!("Recording restarted successfully"),
                Err(e) => error!("Failed to restart recording after save: {}", e),
            }
        } else {
            self.state = RecorderState::Idle;
        }
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        info!("Recorder shutting down");
        self.stop();
    }
}

// ── Helpers ────────────────────────────────────────────────────────

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

/// Wait for the file system to flush the last segment after FFmpeg exits.
/// Polls up to 500ms instead of a fixed 200ms sleep.
fn wait_for_segments_flush() {
    let deadline = Instant::now() + Duration::from_millis(500);
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Extract the numeric index from a segment filename like `seg_0003.ts`.
fn extract_segment_index(path: &Path) -> u32 {
    path.file_stem()
        .and_then(|s| s.to_str())
        .and_then(|s| s.strip_prefix("seg_"))
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(u32::MAX)
}

/// Reorder wrapped segments so the oldest segment is first.
/// When segment_wrap is active, FFmpeg overwrites old segments in a ring.
/// The newest segment has the highest mtime; the segment right after it
/// (by index) is the oldest one still on disk.
fn reorder_wrapped_segments(segments: &mut Vec<PathBuf>) {
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
                "-loglevel", "error",
                "-f", "lavfi",
                "-i", "color=c=black:s=256x256:d=0.04,format=nv12",
                "-frames:v", "1",
                "-c:v", ffmpeg_name,
                "-f", "null",
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
                warn!("Hardware encoder {} compiled in but not usable: {}", label, reason);
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
    quality: &Quality,
    encoding_threads: &str,
    crf: &str,
    force_kf: &str,
) {
    match encoder {
        VideoEncoder::Libx264 => {
            args.extend([
                "-c:v".into(),
                "libx264".into(),
                "-threads".into(),
                encoding_threads.to_string(),
                "-preset".into(),
                quality.preset().into(),
                "-crf".into(),
                crf.into(),
                "-pix_fmt".into(),
                "yuv420p".into(),
                "-force_key_frames".into(),
                force_kf.into(),
            ]);
        }
        VideoEncoder::H264Nvenc => {
            let bitrate = format!("{}k", quality.target_bitrate_kbps());
            let maxrate = format!("{}k", quality.target_bitrate_kbps() * 2);
            let bufsize = format!("{}k", quality.target_bitrate_kbps() * 2);
            args.extend([
                "-c:v".into(),
                "h264_nvenc".into(),
                "-preset".into(),
                quality.nvenc_preset().into(),
                "-b:v".into(),
                bitrate,
                "-maxrate".into(),
                maxrate,
                "-bufsize".into(),
                bufsize,
                "-pix_fmt".into(),
                "yuv420p".into(),
                "-force_key_frames".into(),
                force_kf.into(),
            ]);
        }
        VideoEncoder::H264Qsv => {
            let bitrate = format!("{}k", quality.target_bitrate_kbps());
            let maxrate = format!("{}k", quality.target_bitrate_kbps() * 2);
            let bufsize = format!("{}k", quality.target_bitrate_kbps() * 2);
            args.extend([
                "-c:v".into(),
                "h264_qsv".into(),
                "-preset".into(),
                quality.qsv_preset().into(),
                "-b:v".into(),
                bitrate,
                "-maxrate".into(),
                maxrate,
                "-bufsize".into(),
                bufsize,
                "-pix_fmt".into(),
                "yuv420p".into(),
                "-force_key_frames".into(),
                force_kf.into(),
            ]);
        }
        VideoEncoder::H264Amf => {
            let bitrate = format!("{}k", quality.target_bitrate_kbps());
            let maxrate = format!("{}k", quality.target_bitrate_kbps() * 2);
            let bufsize = format!("{}k", quality.target_bitrate_kbps() * 2);
            args.extend([
                "-c:v".into(),
                "h264_amf".into(),
                "-quality".into(),
                quality.amf_quality().into(),
                "-b:v".into(),
                bitrate,
                "-maxrate".into(),
                maxrate,
                "-bufsize".into(),
                bufsize,
                "-pix_fmt".into(),
                "yuv420p".into(),
                "-force_key_frames".into(),
                force_kf.into(),
            ]);
        }
        VideoEncoder::Auto => {} // resolved before reaching here
    }
}
