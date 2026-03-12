use anyhow::{bail, Context, Result};
use image::RgbaImage;
use rodio::{OutputStream, Sink, Source};
use std::collections::VecDeque;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use super::frame_cache::FrameCache;
use crate::output::functions::ffmpeg_executable_path;
use crate::output::VideoFileMetadata;

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x08000000;

const CACHE_MEMORY_MB: usize = 200;
const KEYFRAME_SCRUB_THRESHOLD_SECS: f64 = 1.0;
/// Maximum frames buffered ahead during live playback.
const PLAYBACK_QUEUE_DEPTH: usize = 24;
/// How long after the last scrub_to() call before we restart FFmpeg (debounce).
const SCRUB_SETTLE_MS: u64 = 150;

pub struct PlaybackFrame {
    pub image: RgbaImage,
    pub from_cache: bool,
}

struct TimedFrame {
    pts_secs: f64,
    image: RgbaImage,
}

pub struct PlaybackController {
    video_path: PathBuf,
    metadata: VideoFileMetadata,
    preview_width: u32,
    shared: Arc<SharedPlaybackState>,
}

struct SharedPlaybackState {
    current_time_secs: Mutex<f64>,
    playing_since: Mutex<Option<PlaybackClock>>,
    latest_frame: Mutex<Option<PlaybackFrame>>,
    frame_queue: Mutex<VecDeque<TimedFrame>>,
    last_error: Mutex<Option<String>>,
    child: Mutex<Option<Child>>,
    generation: AtomicU64,
    audio_loading: AtomicBool,
    audio_buffer: Mutex<Option<AudioBuffer>>,
    audio_generation: AtomicU64,
    audio_started_generation: AtomicU64,
    frame_cache: Mutex<FrameCache>,
    keyframe_positions: Mutex<Vec<f64>>,
    scrub_settle_deadline: Mutex<Option<Instant>>,
}

struct PlaybackClock {
    start_time_secs: f64,
    started_at: Instant,
}

struct AudioBuffer {
    sample_rate: u32,
    channels: u16,
    samples: Arc<Vec<i16>>,
}

struct AudioSliceSource {
    samples: Arc<Vec<i16>>,
    next_index: usize,
    channels: u16,
    sample_rate: u32,
}

impl PlaybackController {
    pub fn new(video_path: PathBuf, metadata: VideoFileMetadata, preview_width: u32) -> Self {
        let shared = Arc::new(SharedPlaybackState {
            current_time_secs: Mutex::new(0.0),
            playing_since: Mutex::new(None),
            latest_frame: Mutex::new(None),
            frame_queue: Mutex::new(VecDeque::new()),
            last_error: Mutex::new(None),
            child: Mutex::new(None),
            generation: AtomicU64::new(0),
            audio_loading: AtomicBool::new(false),
            audio_buffer: Mutex::new(None),
            audio_generation: AtomicU64::new(1),
            audio_started_generation: AtomicU64::new(0),
            frame_cache: Mutex::new(FrameCache::new(CACHE_MEMORY_MB)),
            keyframe_positions: Mutex::new(Vec::new()),
            scrub_settle_deadline: Mutex::new(None),
        });

        let controller = Self {
            video_path,
            metadata,
            preview_width,
            shared,
        };

        controller.begin_audio_preload();
        controller.begin_keyframe_extraction();
        controller
    }

    fn set_latest_frame_from_cache(&self, time_secs: f64, tolerance_secs: f64) -> bool {
        let cached = self
            .shared
            .frame_cache
            .lock()
            .unwrap()
            .get_closest(time_secs, tolerance_secs);
        let Some(cached) = cached else {
            return false;
        };
        let Some(image) =
            RgbaImage::from_raw(cached.width, cached.height, (*cached.rgba_data).clone())
        else {
            return false;
        };
        *self.shared.latest_frame.lock().unwrap() = Some(PlaybackFrame {
            image,
            from_cache: true,
        });
        true
    }

    pub fn request_preview_frame(&mut self, time_secs: f64) {
        let clamped_time = self.clamp_time(time_secs);
        self.pause_at(clamped_time);

        if self.set_latest_frame_from_cache(clamped_time, 0.0) {
            return;
        }

        self.spawn_video_process(clamped_time, clamped_time, true, false);
    }

    pub fn request_preview_frame_fast(&mut self, time_secs: f64) {
        let clamped_time = self.clamp_time(time_secs);

        let _ = self.set_latest_frame_from_cache(clamped_time, 0.5);

        self.pause_at(clamped_time);

        let seek_time = self
            .find_nearest_keyframe(clamped_time)
            .unwrap_or(clamped_time);
        self.spawn_video_process(seek_time, clamped_time, true, true);
    }

    pub fn play_from(&mut self, time_secs: f64) {
        let clamped_time = self.clamp_time(time_secs);
        self.stop_video_process();
        self.stop_audio();
        self.shared.frame_queue.lock().unwrap().clear();
        *self.shared.scrub_settle_deadline.lock().unwrap() = None;
        *self.shared.current_time_secs.lock().unwrap() = clamped_time;
        *self.shared.playing_since.lock().unwrap() = Some(PlaybackClock {
            start_time_secs: clamped_time,
            started_at: Instant::now(),
        });
        self.spawn_playback_process(clamped_time);
        self.maybe_start_audio();
    }

    pub fn pause_at(&mut self, time_secs: f64) {
        let clamped_time = self.clamp_time(time_secs);
        *self.shared.current_time_secs.lock().unwrap() = clamped_time;
        *self.shared.playing_since.lock().unwrap() = None;
        *self.shared.scrub_settle_deadline.lock().unwrap() = None;
        self.shared.frame_queue.lock().unwrap().clear();
        self.stop_video_process();
        self.stop_audio();
    }

    /// Scrub to a new position.
    ///
    /// If paused: stops the current process and issues a single-frame preview
    /// via the existing latest_frame path.
    ///
    /// If playing: immediately updates the wall-clock position and schedules a
    /// debounced FFmpeg restart. The frame queue is cleared so stale frames are
    /// not displayed. The caller should continue using `take_playback_frame` —
    /// a new FFmpeg process will start automatically once scrubbing settles.
    pub fn scrub_to(&mut self, time_secs: f64) {
        let clamped_time = self.clamp_time(time_secs);

        if !self.is_playing() {
            self.stop_video_process();
            self.stop_audio();
            *self.shared.current_time_secs.lock().unwrap() = clamped_time;
            return;
        }

        // Update clock to new position immediately.
        *self.shared.current_time_secs.lock().unwrap() = clamped_time;
        *self.shared.playing_since.lock().unwrap() = Some(PlaybackClock {
            start_time_secs: clamped_time,
            started_at: Instant::now(),
        });

        let _ = self.set_latest_frame_from_cache(clamped_time, 0.5);

        // Discard buffered frames from the old position.
        self.shared.frame_queue.lock().unwrap().clear();

        // Kill the running FFmpeg process so it stops producing frames from
        // the wrong position, but don't restart yet — wait for scrub to settle.
        self.stop_video_process();
        self.stop_audio();

        // Schedule a restart after the scrub settle delay.
        *self.shared.scrub_settle_deadline.lock().unwrap() =
            Some(Instant::now() + Duration::from_millis(SCRUB_SETTLE_MS));
    }

    /// Immediately restart playback from the given position.  Call this when
    /// a scrub drag ends so playback resumes without waiting for the settle
    /// timer.
    pub fn commit_scrub(&mut self, time_secs: f64) {
        if !self.is_playing() {
            return;
        }
        let clamped_time = self.clamp_time(time_secs);
        *self.shared.scrub_settle_deadline.lock().unwrap() = None;
        self.stop_video_process();
        self.stop_audio();
        self.shared.frame_queue.lock().unwrap().clear();
        *self.shared.current_time_secs.lock().unwrap() = clamped_time;
        *self.shared.playing_since.lock().unwrap() = Some(PlaybackClock {
            start_time_secs: clamped_time,
            started_at: Instant::now(),
        });
        self.spawn_playback_process(clamped_time);
        self.maybe_start_audio();
    }

    pub fn playback_position_secs(&self) -> f64 {
        let maybe_clock = self.shared.playing_since.lock().unwrap();
        if let Some(clock) = maybe_clock.as_ref() {
            return self
                .clamp_time(clock.start_time_secs + clock.started_at.elapsed().as_secs_f64());
        }
        *self.shared.current_time_secs.lock().unwrap()
    }

    pub fn is_playing(&self) -> bool {
        self.shared.playing_since.lock().unwrap().is_some()
    }

    pub fn has_pending_activity(&self) -> bool {
        self.is_playing()
            || self.shared.audio_loading.load(Ordering::SeqCst)
            || self.shared.child.lock().unwrap().is_some()
            || self.shared.scrub_settle_deadline.lock().unwrap().is_some()
    }

    /// Take a single-frame preview result (used in paused / static-seek mode).
    pub fn take_frame(&self) -> Option<PlaybackFrame> {
        self.shared.latest_frame.lock().unwrap().take()
    }

    /// During live playback, drain the frame queue up to `wall_time_secs` and
    /// return the most recent frame whose PTS is at or before that time.
    /// Returns `None` if no frame is ready yet.
    pub fn take_playback_frame(&self, wall_time_secs: f64) -> Option<RgbaImage> {
        let mut queue = self.shared.frame_queue.lock().unwrap();
        let mut result: Option<RgbaImage> = None;

        // Pop all frames that should have already been shown.
        while queue.front().map(|f| f.pts_secs <= wall_time_secs).unwrap_or(false) {
            if let Some(f) = queue.pop_front() {
                result = Some(f.image);
            }
        }

        result
    }

    pub fn take_error(&self) -> Option<String> {
        self.shared.last_error.lock().unwrap().take()
    }

    /// Returns the video FPS clamped to a sensible playback range.
    pub fn playback_fps(&self) -> f64 {
        self.metadata.fps.clamp(1.0, 120.0)
    }

    pub fn poll(&mut self) {
        if !self.is_playing() {
            return;
        }

        // Check if a debounced scrub has settled and restart FFmpeg.
        let should_restart = {
            let mut deadline = self.shared.scrub_settle_deadline.lock().unwrap();
            match *deadline {
                Some(d) if Instant::now() >= d => {
                    *deadline = None;
                    true
                }
                _ => false,
            }
        };

        if should_restart {
            let time = self.playback_position_secs();
            self.stop_video_process();
            self.shared.frame_queue.lock().unwrap().clear();
            self.spawn_playback_process(time);
            self.maybe_start_audio();
            return;
        }

        let current_time = self.playback_position_secs();
        *self.shared.current_time_secs.lock().unwrap() = current_time;

        if current_time >= self.metadata.duration_secs {
            self.pause_at(self.metadata.duration_secs);
        } else {
            self.maybe_start_audio();
        }
    }

    pub fn cache_stats(&self) -> (usize, f64) {
        let cache = self.shared.frame_cache.lock().unwrap();
        (cache.entry_count(), cache.memory_usage_mb())
    }

    fn find_nearest_keyframe(&self, time_secs: f64) -> Option<f64> {
        let positions = self.shared.keyframe_positions.lock().unwrap();
        if positions.is_empty() {
            return None;
        }

        let mut nearest = positions[0];
        let mut min_diff = (positions[0] - time_secs).abs();

        for &pos in positions.iter() {
            let diff = (pos - time_secs).abs();
            if diff < min_diff {
                min_diff = diff;
                nearest = pos;
            }
        }

        if (nearest - time_secs).abs() > KEYFRAME_SCRUB_THRESHOLD_SECS {
            return None;
        }

        Some(nearest)
    }

    fn begin_keyframe_extraction(&self) {
        let video_path = self.video_path.clone();
        let shared = Arc::clone(&self.shared);

        thread::spawn(move || {
            let ffmpeg = ffmpeg_executable_path();
            let mut command = Command::new(&ffmpeg);
            command.args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-i",
                &video_path.to_string_lossy(),
                "-vf",
                "showinfo",
                "-f",
                "null",
                "-",
            ]);
            command.stderr(Stdio::piped());

            #[cfg(target_os = "windows")]
            {
                use std::os::windows::process::CommandExt;
                command.creation_flags(CREATE_NO_WINDOW);
            }

            if let Ok(output) = command.output() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let mut positions = Vec::new();

                for line in stderr.lines() {
                    if line.contains("key:1") || line.contains("is_key") {
                        if let Some(pts_start) = line.find("pts_time:") {
                            let pts_str = &line[pts_start + 9..];
                            if let Some(end) =
                                pts_str.find(|c: char| !c.is_numeric() && c != '.' && c != '-')
                            {
                                if let Ok(pts) = pts_str[..end].parse::<f64>() {
                                    if pts >= 0.0 {
                                        positions.push(pts);
                                    }
                                }
                            }
                        }
                    }
                }

                if !positions.is_empty() {
                    positions.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                    positions.dedup_by(|a, b| (*a - *b).abs() < 0.01);
                }

                *shared.keyframe_positions.lock().unwrap() = positions;
            }
        });
    }

    fn begin_audio_preload(&self) {
        if !self.metadata.has_audio || self.shared.audio_loading.swap(true, Ordering::SeqCst) {
            return;
        }

        let video_path = self.video_path.clone();
        let shared = self.shared.clone();
        thread::spawn(move || {
            let result = decode_audio_track(&video_path);
            match result {
                Ok(buffer) => {
                    *shared.audio_buffer.lock().unwrap() = Some(buffer);
                }
                Err(err) => {
                    *shared.last_error.lock().unwrap() =
                        Some(format!("Audio preload failed: {err:#}"));
                }
            }
            shared.audio_loading.store(false, Ordering::SeqCst);
        });
    }

    fn maybe_start_audio(&mut self) {
        let Some(buffer) = self
            .shared
            .audio_buffer
            .lock()
            .unwrap()
            .as_ref()
            .map(clone_audio_buffer)
        else {
            return;
        };

        let current_audio_generation = self.shared.audio_generation.load(Ordering::SeqCst);
        if self.shared.audio_started_generation.load(Ordering::SeqCst) == current_audio_generation {
            return;
        }

        let start_time = self.playback_position_secs();
        let generation = self.shared.audio_generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.shared
            .audio_started_generation
            .store(generation, Ordering::SeqCst);
        let shared = self.shared.clone();
        thread::spawn(move || {
            let (stream, handle) = match OutputStream::try_default() {
                Ok(stream_and_handle) => stream_and_handle,
                Err(err) => {
                    *shared.last_error.lock().unwrap() =
                        Some(format!("Audio output failed: {err}"));
                    return;
                }
            };
            let sink = match Sink::try_new(&handle) {
                Ok(sink) => sink,
                Err(err) => {
                    *shared.last_error.lock().unwrap() =
                        Some(format!("Audio output failed: {err}"));
                    drop(stream);
                    return;
                }
            };
            sink.append(AudioSliceSource::new(buffer, start_time));
            loop {
                if shared.audio_generation.load(Ordering::SeqCst) != generation {
                    sink.stop();
                    break;
                }
                if sink.empty() {
                    break;
                }
                thread::sleep(Duration::from_millis(20));
            }
            drop(sink);
            drop(stream);
        });
    }

    fn stop_audio(&mut self) {
        self.shared.audio_generation.fetch_add(1, Ordering::SeqCst);
    }

    /// Spawn a continuous playback FFmpeg process that pushes decoded frames
    /// into the shared `frame_queue` with wall-clock-aligned PTS values.
    fn spawn_playback_process(&mut self, seek_time: f64) {
        self.stop_video_process();
        let generation = self.shared.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let ffmpeg = ffmpeg_executable_path();
        let (out_width, out_height) = self.scaled_dimensions();
        let frame_len = out_width as usize * out_height as usize * 4;
        let timestamp = format_seconds_arg(seek_time);
        let fps = self.playback_fps();

        let mut command = Command::new(&ffmpeg);
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        command.args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-ss",
            &timestamp,
            "-i",
            &self.video_path.to_string_lossy(),
            "-an",
            "-vf",
            &format!("scale={out_width}:{out_height}"),
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgba",
            "-",
        ]);

        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(CREATE_NO_WINDOW);
        }

        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(err) => {
                *self.shared.last_error.lock().unwrap() =
                    Some(format!("Playback process failed: {err}"));
                return;
            }
        };

        let stdout = match child.stdout.take() {
            Some(s) => s,
            None => {
                *self.shared.last_error.lock().unwrap() =
                    Some("Playback process stdout was unavailable".to_string());
                let _ = child.kill();
                let _ = child.wait();
                return;
            }
        };
        let stderr = child.stderr.take();
        *self.shared.child.lock().unwrap() = Some(child);

        let shared = self.shared.clone();
        let frame_duration = 1.0 / fps;

        thread::spawn(move || {
            let mut stdout = stdout;
            let mut buffer = vec![0_u8; frame_len];
            let mut frame_index: u64 = 0;
            let mut first_frame = true;

            loop {
                // Back-pressure: pause reading when the queue is full so we
                // don't decode far ahead of the presentation position.
                loop {
                    if shared.generation.load(Ordering::SeqCst) != generation {
                        return;
                    }
                    if shared.frame_queue.lock().unwrap().len() < PLAYBACK_QUEUE_DEPTH {
                        break;
                    }
                    thread::sleep(Duration::from_millis(2));
                }

                if shared.generation.load(Ordering::SeqCst) != generation {
                    break;
                }

                match stdout.read_exact(&mut buffer) {
                    Ok(()) => {
                        if shared.generation.load(Ordering::SeqCst) != generation {
                            break;
                        }
                        if let Some(image) =
                            RgbaImage::from_raw(out_width, out_height, buffer.clone())
                        {
                            let pts = seek_time + frame_index as f64 * frame_duration;

                            shared.frame_cache.lock().unwrap().insert(
                                pts,
                                buffer.clone(),
                                out_width,
                                out_height,
                            );
                            if first_frame {
                                first_frame = false;
                            }

                            shared
                                .frame_queue
                                .lock()
                                .unwrap()
                                .push_back(TimedFrame { pts_secs: pts, image });
                        }
                        frame_index += 1;
                    }
                    Err(err) => {
                        if first_frame && shared.generation.load(Ordering::SeqCst) == generation {
                            if let Some(mut stderr) = stderr {
                                let mut stderr_text = String::new();
                                let _ = stderr.read_to_string(&mut stderr_text);
                                *shared.last_error.lock().unwrap() =
                                    Some(if stderr_text.trim().is_empty() {
                                        format!("Playback stream failed: {err}")
                                    } else {
                                        format!(
                                            "Playback stream failed: {}",
                                            stderr_text.trim()
                                        )
                                    });
                            }
                        }
                        break;
                    }
                }
            }

            if shared.generation.load(Ordering::SeqCst) == generation {
                if let Some(mut child) = shared.child.lock().unwrap().take() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
            }
        });
    }

    fn spawn_video_process(
        &mut self,
        seek_time: f64,
        display_time: f64,
        single_frame: bool,
        low_priority: bool,
    ) {
        self.stop_video_process();
        let generation = self.shared.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let ffmpeg = ffmpeg_executable_path();
        let (out_width, out_height) = self.scaled_dimensions();
        let frame_len = out_width as usize * out_height as usize * 4;
        let timestamp = format_seconds_arg(seek_time);
        let mut command = Command::new(&ffmpeg);
        command.stdout(Stdio::piped()).stderr(Stdio::piped());

        if single_frame {
            command.args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-ss",
                &timestamp,
                "-i",
                &self.video_path.to_string_lossy(),
                "-an",
                "-frames:v",
                "1",
                "-vf",
                &format!("scale={out_width}:{out_height}"),
                "-f",
                "rawvideo",
                "-pix_fmt",
                "rgba",
                "-",
            ]);
        } else {
            // Legacy multi-frame path (preview_final mode). Use -re for
            // controlled pacing since presentation timing is handled externally.
            command.args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-re",
                "-ss",
                &timestamp,
                "-i",
                &self.video_path.to_string_lossy(),
                "-an",
                "-vf",
                &format!("scale={out_width}:{out_height}"),
                "-f",
                "rawvideo",
                "-pix_fmt",
                "rgba",
                "-",
            ]);
        }

        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            if low_priority {
                const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x00004000;
                command.creation_flags(CREATE_NO_WINDOW | BELOW_NORMAL_PRIORITY_CLASS);
            } else {
                command.creation_flags(CREATE_NO_WINDOW);
            }
        }

        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(err) => {
                *self.shared.last_error.lock().unwrap() =
                    Some(format!("Preview process failed: {err}"));
                return;
            }
        };

        let stdout = match child.stdout.take() {
            Some(stdout) => stdout,
            None => {
                *self.shared.last_error.lock().unwrap() =
                    Some("Preview process stdout was unavailable".to_string());
                let _ = child.kill();
                let _ = child.wait();
                return;
            }
        };
        let stderr = child.stderr.take();
        *self.shared.child.lock().unwrap() = Some(child);

        let shared = self.shared.clone();
        let cache_time = display_time;

        thread::spawn(move || {
            let mut stdout = stdout;
            let mut buffer = vec![0_u8; frame_len];
            let mut first_frame = true;

            loop {
                match stdout.read_exact(&mut buffer) {
                    Ok(()) => {
                        if shared.generation.load(Ordering::SeqCst) != generation {
                            break;
                        }
                        if let Some(image) =
                            RgbaImage::from_raw(out_width, out_height, buffer.clone())
                        {
                            if first_frame {
                                shared.frame_cache.lock().unwrap().insert(
                                    cache_time,
                                    buffer.clone(),
                                    out_width,
                                    out_height,
                                );
                            }
                            *shared.latest_frame.lock().unwrap() = Some(PlaybackFrame {
                                image,
                                from_cache: false,
                            });
                        }
                        if single_frame {
                            break;
                        }
                        first_frame = false;
                    }
                    Err(err) => {
                        if first_frame && shared.generation.load(Ordering::SeqCst) == generation {
                            if let Some(mut stderr) = stderr {
                                let mut stderr_text = String::new();
                                let _ = stderr.read_to_string(&mut stderr_text);
                                *shared.last_error.lock().unwrap() =
                                    Some(if stderr_text.trim().is_empty() {
                                        format!("Preview stream failed: {err}")
                                    } else {
                                        format!("Preview stream failed: {}", stderr_text.trim())
                                    });
                            }
                        }
                        break;
                    }
                }
            }

            if shared.generation.load(Ordering::SeqCst) == generation {
                if let Some(mut child) = shared.child.lock().unwrap().take() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
            }
        });
    }

    fn stop_video_process(&self) {
        self.shared.generation.fetch_add(1, Ordering::SeqCst);
        if let Some(mut child) = self.shared.child.lock().unwrap().take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    fn clamp_time(&self, time_secs: f64) -> f64 {
        time_secs.clamp(0.0, self.metadata.duration_secs)
    }

    fn scaled_dimensions(&self) -> (u32, u32) {
        let width = self.preview_width.min(self.metadata.width.max(1)).max(1);
        let aspect = self.metadata.height.max(1) as f64 / self.metadata.width.max(1) as f64;
        let mut height = (f64::from(width) * aspect).round() as u32;
        height = height.max(1);
        if height % 2 != 0 {
            height += 1;
        }
        (width, height)
    }
}

impl Drop for PlaybackController {
    fn drop(&mut self) {
        self.pause_at(self.playback_position_secs());
    }
}

impl AudioSliceSource {
    fn new(buffer: AudioBuffer, start_time_secs: f64) -> Self {
        let channel_count = usize::from(buffer.channels.max(1));
        let start_frames = (start_time_secs.max(0.0) * buffer.sample_rate as f64).round() as usize;
        let start_index = (start_frames * channel_count).min(buffer.samples.len());
        Self {
            samples: buffer.samples,
            next_index: start_index,
            channels: buffer.channels,
            sample_rate: buffer.sample_rate,
        }
    }
}

impl Iterator for AudioSliceSource {
    type Item = i16;

    fn next(&mut self) -> Option<Self::Item> {
        if self.next_index >= self.samples.len() {
            return None;
        }
        let sample = self.samples[self.next_index];
        self.next_index += 1;
        Some(sample)
    }
}

impl Source for AudioSliceSource {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> u16 {
        self.channels
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

fn clone_audio_buffer(buffer: &AudioBuffer) -> AudioBuffer {
    AudioBuffer {
        sample_rate: buffer.sample_rate,
        channels: buffer.channels,
        samples: buffer.samples.clone(),
    }
}

fn decode_audio_track(video_path: &PathBuf) -> Result<AudioBuffer> {
    let ffmpeg = ffmpeg_executable_path();
    let mut command = Command::new(&ffmpeg);
    command.args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-i",
        &video_path.to_string_lossy(),
        "-vn",
        "-f",
        "s16le",
        "-acodec",
        "pcm_s16le",
        "-ac",
        "2",
        "-ar",
        "48000",
        "-",
    ]);

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    let output = command
        .output()
        .with_context(|| format!("Failed to decode audio via {:?}", ffmpeg))?;

    if !output.status.success() {
        bail!(
            "ffmpeg audio decode failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    let mut samples = Vec::with_capacity(output.stdout.len() / 2);
    for chunk in output.stdout.chunks_exact(2) {
        samples.push(i16::from_le_bytes([chunk[0], chunk[1]]));
    }

    Ok(AudioBuffer {
        sample_rate: 48_000,
        channels: 2,
        samples: Arc::new(samples),
    })
}

fn format_seconds_arg(seconds: f64) -> String {
    let total_millis = (seconds.max(0.0) * 1000.0).round() as u64;
    let hours = total_millis / 3_600_000;
    let minutes = (total_millis / 60_000) % 60;
    let secs = (total_millis / 1000) % 60;
    let millis = total_millis % 1000;
    format!("{hours:02}:{minutes:02}:{secs:02}.{millis:03}")
}
