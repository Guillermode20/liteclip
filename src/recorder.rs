use log::{error, info, warn};
use std::collections::VecDeque;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tempfile::TempDir;

use crate::settings::{Resolution, Settings, VideoEncoder};

#[cfg(windows)]
use windows_capture::capture::GraphicsCaptureApiHandler;
#[cfg(windows)]
use windows_capture::encoder::{
    AudioSettingsBuilder, ContainerSettingsBuilder, ContainerSettingsSubType,
    VideoEncoder as NativeEncoder, VideoSettingsBuilder, VideoSettingsSubType,
};
#[cfg(windows)]
use windows_capture::frame::Frame;
#[cfg(windows)]
use windows_capture::graphics_capture_api::InternalCaptureControl;
#[cfg(windows)]
use windows_capture::monitor::Monitor;
#[cfg(windows)]
use windows_capture::settings::{
    ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings,
    MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings as NativeSettings,
};

const SEGMENT_DURATION_SECS: u64 = 4;
const MIN_STABLE_SEGMENT_BYTES: u64 = 8 * 1024; // 8KB minimum

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecorderState {
    Idle,
    Recording,
    Saving,
}

#[derive(Debug)]
struct WorkerState {
    stop_flag: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

pub struct Recorder {
    pub state: RecorderState,
    pub settings: Settings,
    pub backend_available: bool,
    pub last_saved_path: Option<PathBuf>,
    pub audio_devices: Vec<String>,
    pub video_encoders: Vec<VideoEncoder>,

    temp_dir: Option<TempDir>,
    started_at: Option<Instant>,
    recording_expected: bool,
    restart_attempts: u32,
    next_restart_attempt_at: Option<Instant>,
    worker: Option<WorkerState>,
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

#[cfg(windows)]
#[derive(Clone)]
struct CaptureFlags {
    output_path: PathBuf,
    width: u32,
    height: u32,
    fps: u32,
    bitrate_bps: u32,
    capture_audio: bool,
    segment_duration: Duration,
    stop_flag: Arc<AtomicBool>,
}

#[cfg(windows)]
struct NativeCapture {
    encoder: Option<NativeEncoder>,
    start_time: Instant,
    segment_duration: Duration,
    stop_flag: Arc<AtomicBool>,
    frame_count: u64,
}

#[cfg(windows)]
impl GraphicsCaptureApiHandler for NativeCapture {
    type Flags = CaptureFlags;
    type Error = String;

    fn new(ctx: windows_capture::capture::Context<Self::Flags>) -> Result<Self, Self::Error> {
        let flags = ctx.flags;
        let video_settings = VideoSettingsBuilder::new(flags.width, flags.height)
            .sub_type(VideoSettingsSubType::H264)
            .frame_rate(flags.fps)
            .bitrate(flags.bitrate_bps);
        let container_settings =
            ContainerSettingsBuilder::default().sub_type(ContainerSettingsSubType::MPEG2);

        let encoder = if flags.capture_audio {
            let audio_settings = AudioSettingsBuilder::default();
            NativeEncoder::new(
                video_settings,
                audio_settings,
                container_settings,
                &flags.output_path,
            )
            .map_err(|e| format!("Failed to create native encoder (with audio): {e}"))?
        } else {
            let audio_settings = AudioSettingsBuilder::default().disabled(true);
            NativeEncoder::new(
                video_settings,
                audio_settings,
                container_settings,
                &flags.output_path,
            )
            .map_err(|e| format!("Failed to create native encoder (video only): {e}"))?
        };

        info!(
            "Native encoder created for segment: {:?} (audio: {}, {}x{} @ {}fps)",
            flags.output_path, flags.capture_audio, flags.width, flags.height, flags.fps
        );

        Ok(Self {
            encoder: Some(encoder),
            start_time: Instant::now(),
            segment_duration: flags.segment_duration,
            stop_flag: flags.stop_flag,
            frame_count: 0,
        })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut Frame,
        capture_control: InternalCaptureControl,
    ) -> Result<(), Self::Error> {
        // Check if segment duration elapsed or external stop requested
        if self.stop_flag.load(Ordering::Relaxed)
            || self.start_time.elapsed() >= self.segment_duration
        {
            info!(
                "Segment capture complete: {} frames in {:.1}s",
                self.frame_count,
                self.start_time.elapsed().as_secs_f64()
            );
            capture_control.stop();
            return Ok(());
        }

        if let Some(encoder) = self.encoder.as_mut() {
            encoder
                .send_frame(frame)
                .map_err(|e| format!("Failed to encode frame: {e}"))?;
        }
        self.frame_count += 1;
        Ok(())
    }

    fn on_closed(&mut self) -> Result<(), Self::Error> {
        info!("Finalizing segment encoder ({} frames)", self.frame_count);
        if let Some(encoder) = self.encoder.take() {
            encoder
                .finish()
                .map_err(|e| format!("Failed to finalize native encoder: {e}"))?;
        }
        Ok(())
    }
}

impl Recorder {
    pub fn new() -> Self {
        let settings = Settings::load();

        #[cfg(windows)]
        let backend_available = true;
        #[cfg(not(windows))]
        let backend_available = false;

        if backend_available {
            info!("Windows native capture backend enabled");
        } else {
            error!("Windows native capture backend unavailable on this platform");
        }

        Self {
            state: RecorderState::Idle,
            settings,
            backend_available,
            last_saved_path: None,
            audio_devices: Vec::new(),
            video_encoders: vec![VideoEncoder::Auto],
            temp_dir: None,
            started_at: None,
            recording_expected: false,
            restart_attempts: 0,
            next_restart_attempt_at: None,
            worker: None,
        }
    }

    fn segment_wrap(&self) -> usize {
        let requested = self.settings.buffer_seconds.max(1);
        let segments = requested.div_ceil(SEGMENT_DURATION_SECS).max(1);
        segments as usize
    }

    pub fn start(&mut self) -> Result<(), String> {
        if !self.backend_available {
            self.recording_expected = false;
            self.restart_attempts = 0;
            self.next_restart_attempt_at = None;
            return Err("Windows native capture is unavailable on this system.".into());
        }

        if self.state == RecorderState::Recording {
            return Err("Already recording.".into());
        }

        let temp_dir = TempDir::new().map_err(|e| format!("Failed to create temp dir: {e}"))?;
        let segment_root = temp_dir.path().to_path_buf();
        let wrap = self.segment_wrap();
        let settings = self.settings.clone();

        let stop_flag = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop_flag);

        let handle = std::thread::spawn(move || {
            native_segment_worker(segment_root, wrap, settings, worker_stop);
        });

        self.temp_dir = Some(temp_dir);
        self.worker = Some(WorkerState {
            stop_flag,
            handle: Some(handle),
        });
        self.started_at = Some(Instant::now());
        self.state = RecorderState::Recording;
        self.recording_expected = true;
        self.restart_attempts = 0;
        self.next_restart_attempt_at = None;
        Ok(())
    }

    pub fn stop(&mut self) {
        if let Some(mut worker) = self.worker.take() {
            worker.stop_flag.store(true, Ordering::SeqCst);
            if let Some(handle) = worker.handle.take() {
                let _ = handle.join();
            }
        }

        self.state = RecorderState::Idle;
        self.started_at = None;
        self.temp_dir = None;
        self.recording_expected = false;
        self.restart_attempts = 0;
        self.next_restart_attempt_at = None;
    }

    pub fn save_clip_auto_detached(recorder: &Arc<Mutex<Recorder>>) -> Result<PathBuf, String> {
        let job = {
            let mut rec = recorder
                .lock()
                .map_err(|_| "Recorder lock poisoned while preparing save.".to_string())?;
            let output_path = rec.auto_output_path();
            rec.prepare_save_job(output_path)?
        };

        let execution = execute_save_job(job);

        let mut rec = recorder
            .lock()
            .map_err(|_| "Recorder lock poisoned while finalizing save.".to_string())?;
        rec.finish_save_job(execution)
    }

    #[allow(dead_code)]
    pub fn save_clip_auto(&mut self) -> Result<PathBuf, String> {
        let output_path = self.auto_output_path();
        self.save_clip(&output_path)
    }

    #[allow(dead_code)]
    pub fn save_clip(&mut self, output_path: &Path) -> Result<PathBuf, String> {
        let save_job = self.prepare_save_job(output_path.to_path_buf())?;
        let execution = execute_save_job(save_job);
        self.finish_save_job(execution)
    }

    pub fn health_check_tick(&mut self) {
        if self.state == RecorderState::Recording {
            let worker_finished = self
                .worker
                .as_ref()
                .and_then(|worker| worker.handle.as_ref())
                .map(std::thread::JoinHandle::is_finished)
                .unwrap_or(true);

            if worker_finished {
                self.mark_unexpected_stop("Native capture worker stopped unexpectedly");
            } else {
                self.restart_attempts = 0;
                self.next_restart_attempt_at = None;
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
            warn!("Attempting recorder auto-restart (attempt #{attempt_number})");

            match self.start() {
                Ok(()) => {
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

    fn auto_output_path(&self) -> PathBuf {
        let now = chrono::Local::now();
        let filename = format!("LiteClipReplay_{}.ts", now.format("%Y-%m-%d_%H-%M-%S"));
        self.settings.output_dir.join(filename)
    }

    fn prepare_save_job(&mut self, output_path: PathBuf) -> Result<SaveJob, String> {
        if self.state == RecorderState::Saving {
            return Err("Save already in progress.".into());
        }

        let resume_state = self.state;

        let segment_root = self
            .temp_dir
            .as_ref()
            .ok_or_else(|| "No recording buffer available yet.".to_string())?
            .path()
            .to_path_buf();

        info!(
            "DEBUG: prepare_save_job called. segment_root={:?}",
            segment_root
        );

        let segment_paths = collect_segments_for_snapshot(&segment_root)?;

        info!(
            "DEBUG: collect_segments_for_snapshot returned {} segments",
            segment_paths.len()
        );

        if segment_paths.is_empty() {
            return Err(
                "Replay buffer is still warming up. Wait a few seconds, then try save again."
                    .into(),
            );
        }

        self.state = RecorderState::Saving;

        Ok(SaveJob {
            resume_state,
            output_path,
            segment_paths,
        })
    }

    fn finish_save_job(&mut self, execution: SaveExecution) -> Result<PathBuf, String> {
        self.state =
            if execution.resume_state == RecorderState::Recording && self.recording_expected {
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

        if let Some(mut worker) = self.worker.take() {
            worker.stop_flag.store(true, Ordering::SeqCst);
            if let Some(handle) = worker.handle.take() {
                let _ = handle.join();
            }
        }

        self.started_at = None;
        self.temp_dir = None;
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
        self.stop();
    }
}

fn execute_save_job(save_job: SaveJob) -> SaveExecution {
    let output_path = save_job.output_path.clone();
    let save_result = (|| -> Result<PathBuf, String> {
        let snapshot_dir =
            TempDir::new().map_err(|e| format!("Failed to create snapshot dir: {e}"))?;
        let copied_segments =
            copy_segments_into_snapshot(&save_job.segment_paths, snapshot_dir.path())?;

        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create output directory: {e}"))?;
        }

        // Binary-concatenate MPEG-TS segments.
        // MPEG Transport Stream is specifically designed for concatenation —
        // contiguous .ts files can be joined byte-for-byte to produce a valid stream.
        let output_file = fs::File::create(&output_path)
            .map_err(|e| format!("Failed to create output file: {e}"))?;
        let mut writer = std::io::BufWriter::new(output_file);
        for segment in &copied_segments {
            let data = fs::read(segment)
                .map_err(|e| format!("Failed to read segment {}: {e}", segment.display()))?;
            writer
                .write_all(&data)
                .map_err(|e| format!("Failed to write to output: {e}"))?;
        }
        writer
            .flush()
            .map_err(|e| format!("Failed to flush output: {e}"))?;
        drop(writer);

        let output_size = fs::metadata(&output_path)
            .map_err(|e| format!("Failed to stat output file {}: {e}", output_path.display()))?
            .len();
        if output_size < MIN_STABLE_SEGMENT_BYTES {
            return Err(
                "Saved clip is empty or too short. Wait a few seconds, then try again.".into(),
            );
        }

        Ok(output_path)
    })();

    SaveExecution {
        resume_state: save_job.resume_state,
        save_result,
    }
}

fn collect_segments_for_snapshot(segment_root: &Path) -> Result<Vec<PathBuf>, String> {
    let entries: Vec<_> = fs::read_dir(segment_root)
        .map_err(|e| format!("Failed to read temp dir {}: {e}", segment_root.display()))?
        .filter_map(|e| e.ok())
        .collect();

    // DEBUG: Log all files in temp directory
    info!("DEBUG: Temp dir contents ({} entries):", entries.len());
    for entry in &entries {
        let path = entry.path();
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("no-ext");
        let size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        info!(
            "DEBUG:   File: {:?}, ext: {}, size: {} bytes",
            path.file_name(),
            ext,
            size
        );
    }

    let mut segments: Vec<PathBuf> = entries
        .into_iter()
        .filter_map(|entry| {
            let path = entry.path();
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_lowercase();
            if matches!(ext.as_str(), "ts" | "mpg" | "mp2" | "mp4" | "mpeg") {
                Some(path)
            } else {
                None
            }
        })
        .collect();

    info!(
        "DEBUG: Found {} segments with video extension",
        segments.len()
    );

    segments.sort_unstable_by_key(|p| segment_file_index(p).unwrap_or(u64::MAX));

    segments.retain(|path| is_stable_segment(path));
    info!("DEBUG: {} segments passed stability check", segments.len());
    Ok(segments)
}

fn is_stable_segment(path: &Path) -> bool {
    match fs::metadata(path) {
        Ok(meta) => meta.len() >= MIN_STABLE_SEGMENT_BYTES,
        Err(_) => false,
    }
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

fn segment_file_index(path: &Path) -> Option<u64> {
    let stem = path.file_stem()?.to_str()?;
    let (_, index) = stem.rsplit_once('_')?;
    index.parse::<u64>().ok()
}

#[cfg(windows)]
fn native_segment_worker(
    segment_root: PathBuf,
    wrap: usize,
    settings: Settings,
    stop_flag: Arc<AtomicBool>,
) {
    let mut recent_segments: VecDeque<PathBuf> = VecDeque::with_capacity(wrap.max(1));
    let mut segment_index: u64 = 0;
    let mut consecutive_failures = 0u32;

    info!(
        "DEBUG: Native segment worker started. segment_root={:?}, wrap={}",
        segment_root, wrap
    );

    while !stop_flag.load(Ordering::SeqCst) {
        let segment_path = segment_root.join(format!("seg_{segment_index:04}.ts"));
        info!("DEBUG: Starting capture for segment: {:?}", segment_path);

        match run_native_segment_capture(&settings, &segment_path, &stop_flag) {
            Ok(()) => {
                consecutive_failures = 0;

                // DEBUG: Check if file was actually created
                match fs::metadata(&segment_path) {
                    Ok(meta) => {
                        info!(
                            "DEBUG: Segment created successfully: {:?}, size={} bytes",
                            segment_path,
                            meta.len()
                        );
                    }
                    Err(e) => {
                        warn!(
                            "DEBUG: Segment file not found after capture: {:?}, error: {}",
                            segment_path, e
                        );
                    }
                }

                recent_segments.push_back(segment_path.clone());
                info!(
                    "DEBUG: Added to recent_segments. Total segments in memory: {}",
                    recent_segments.len()
                );

                while recent_segments.len() > wrap {
                    if let Some(oldest) = recent_segments.pop_front() {
                        info!("DEBUG: Removing oldest segment: {:?}", oldest);
                        let _ = fs::remove_file(oldest);
                    }
                }
            }
            Err(e) => {
                consecutive_failures = consecutive_failures.saturating_add(1);
                error!("Native segment capture failed: {e}");
                if consecutive_failures >= 3 {
                    error!("Native segment capture failed repeatedly; worker will stop to allow watchdog restart");
                    break;
                }
                std::thread::sleep(Duration::from_millis(500));
            }
        }

        segment_index = segment_index.saturating_add(1);
    }

    info!("DEBUG: Native segment worker stopped");
}

#[cfg(not(windows))]
fn native_segment_worker(
    _segment_root: PathBuf,
    _wrap: usize,
    _settings: Settings,
    _stop_flag: Arc<AtomicBool>,
) {
}

#[cfg(windows)]
fn run_native_segment_capture(
    settings: &Settings,
    output_path: &Path,
    stop_flag: &Arc<AtomicBool>,
) -> Result<(), String> {
    let monitor = Monitor::primary().map_err(|e| format!("No primary monitor available: {e}"))?;

    let native_width = monitor
        .width()
        .map_err(|e| format!("Failed to read monitor width: {e}"))?;
    let native_height = monitor
        .height()
        .map_err(|e| format!("Failed to read monitor height: {e}"))?;

    let (width, height) = resolve_capture_resolution(settings, native_width, native_height);
    let fps = settings.framerate.value().clamp(15, 60);

    let bitrate_kbps = if settings.advanced_video_controls {
        settings.video_bitrate_kbps.clamp(1000, 150_000)
    } else {
        settings.quality.target_bitrate_kbps()
    };

    let flags = CaptureFlags {
        output_path: output_path.to_path_buf(),
        width,
        height,
        fps,
        bitrate_bps: bitrate_kbps.saturating_mul(1000),
        capture_audio: settings.capture_audio,
        segment_duration: Duration::from_secs(SEGMENT_DURATION_SECS),
        stop_flag: Arc::clone(stop_flag),
    };

    let min_interval_ms = 1000u64 / u64::from(fps.max(1));

    let capture_settings = NativeSettings::new(
        monitor,
        CursorCaptureSettings::WithCursor,
        DrawBorderSettings::WithoutBorder,
        SecondaryWindowSettings::Default,
        MinimumUpdateIntervalSettings::Custom(Duration::from_millis(min_interval_ms.max(1))),
        DirtyRegionSettings::Default,
        ColorFormat::Bgra8,
        flags,
    );

    info!(
        "Starting native capture: {}x{} @ {}fps, output: {:?}",
        width, height, fps, output_path
    );

    // start() blocks until the capture stops (via capture_control.stop() in on_frame_arrived).
    // By the time it returns, on_closed() has been called and the file is fully written.
    NativeCapture::start(capture_settings).map_err(|e| {
        error!("Native capture failed: {}", e);
        format!("Native capture failed: {e}")
    })?;

    Ok(())
}

#[cfg(not(windows))]
fn run_native_segment_capture(
    _settings: &Settings,
    _output_path: &Path,
    _stop_flag: &Arc<AtomicBool>,
) -> Result<(), String> {
    Err("Windows native capture is only available on Windows.".into())
}

fn resolve_capture_resolution(
    settings: &Settings,
    native_width: u32,
    native_height: u32,
) -> (u32, u32) {
    if settings.custom_resolution_enabled {
        return (
            clamp_even(settings.custom_resolution_width, 320, native_width.max(320)),
            clamp_even(
                settings.custom_resolution_height,
                240,
                native_height.max(240),
            ),
        );
    }

    match settings.resolution {
        Resolution::Native => (
            clamp_even(native_width, 320, 7680),
            clamp_even(native_height, 240, 4320),
        ),
        Resolution::Res1080p => scaled_size(native_width, native_height, 1920, 1080),
        Resolution::Res720p => scaled_size(native_width, native_height, 1280, 720),
        Resolution::Res480p => scaled_size(native_width, native_height, 854, 480),
    }
}

fn scaled_size(native_width: u32, native_height: u32, target_w: u32, target_h: u32) -> (u32, u32) {
    if native_width <= target_w && native_height <= target_h {
        return (
            clamp_even(native_width, 320, 7680),
            clamp_even(native_height, 240, 4320),
        );
    }

    let width_ratio = native_width as f64 / target_w as f64;
    let height_ratio = native_height as f64 / target_h as f64;
    let ratio = width_ratio.max(height_ratio).max(1.0);

    let scaled_w = (native_width as f64 / ratio).round() as u32;
    let scaled_h = (native_height as f64 / ratio).round() as u32;

    (
        clamp_even(scaled_w, 320, 7680),
        clamp_even(scaled_h, 240, 4320),
    )
}

fn clamp_even(value: u32, min: u32, max: u32) -> u32 {
    let clamped = value.clamp(min, max);
    if clamped % 2 == 0 {
        clamped
    } else if clamped == max {
        clamped.saturating_sub(1)
    } else {
        clamped + 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restart_backoff_is_capped() {
        assert_eq!(Recorder::restart_backoff_delay(1), Duration::from_secs(1));
        assert_eq!(Recorder::restart_backoff_delay(2), Duration::from_secs(2));
        assert_eq!(Recorder::restart_backoff_delay(3), Duration::from_secs(4));
        assert_eq!(Recorder::restart_backoff_delay(8), Duration::from_secs(30));
    }

    #[test]
    fn clamp_even_keeps_even() {
        assert_eq!(clamp_even(1921, 320, 7680), 1922);
        assert_eq!(clamp_even(1919, 320, 1919), 1918);
    }
}
