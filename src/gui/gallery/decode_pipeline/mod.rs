use anyhow::{anyhow, bail, Context, Result};
use crossbeam_channel::{bounded, Receiver, Sender, TryRecvError};
use ffmpeg_next as ffmpeg;
use ffmpeg_next::packet::Mut;
use rodio::{Sink, Source};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use egui::epaint::ColorImage;

use crate::output::VideoFileMetadata;
use crate::quality_contracts::{
    assess_gallery_playback_runtime, GalleryPlaybackRuntimeSample, GALLERY_PLAYBACK_GUARDRAIL,
};

mod frame_pool;

use frame_pool::{FramePool, PooledColorImage, FRAME_POOL_SIZE};

// -------- Configuration constants -----------------------------------------

/// Frame channel capacity for decoder-to-UI communication.
const FRAME_CHANNEL_CAPACITY: usize = 12;

/// Maximum frames queued for playback (playing state).
const PLAYBACK_QUEUE_DEPTH_PLAYING: usize = 20;

/// Maximum frames queued for playback (paused state — decoder is idle).
const PLAYBACK_QUEUE_DEPTH_PAUSED: usize = 6;

/// Target pool size when the editor is idle/paused.
const FRAME_POOL_IDLE_TRIM_TARGET: usize = 8;

/// Target pool size during active playback.
const FRAME_POOL_ACTIVE_TRIM_TARGET: usize = 24;

/// Maximum drift (in seconds) between two seek targets that we treat as
/// "forward decode" — we continue from the last decoded position instead
/// of seeking to the nearest keyframe. This avoids the keyframe-seek +
/// frame-skip tax during short forward scrubs.
const FORWARD_DECODE_THRESHOLD_SECS: f64 = 1.0;

// -------- Hardware decode context -----------------------------------------

struct DecoderHardwareContext {
    device_ctx_ref: *mut ffmpeg::ffi::AVBufferRef,
}

// SAFETY: AVBufferRef uses atomic RC internally; used from one thread at a time.
unsafe impl Send for DecoderHardwareContext {}

impl Drop for DecoderHardwareContext {
    fn drop(&mut self) {
        unsafe {
            if !self.device_ctx_ref.is_null() {
                ffmpeg::ffi::av_buffer_unref(&mut self.device_ctx_ref);
            }
        }
    }
}

// -------- Public types ----------------------------------------------------

/// A single frame retrieved from the playback controller.
pub struct PlaybackFrame {
    /// Zero-copy ColorImage — pass directly to `texture.set()`.
    pub image: ColorImage,
    pub pts_secs: f64,
}

// -------- Internal types --------------------------------------------------

struct TimedFrame {
    pts_secs: f64,
    image: ColorImage,
}

pub struct PlaybackController {
    metadata: VideoFileMetadata,
    shared: Arc<SharedPlaybackState>,
    decoder: DecodePipeline,
    audio_handle: Option<rodio::OutputStreamHandle>,
    _audio_shutdown_tx: Option<std::sync::mpsc::Sender<()>>,
    audio_stream_thread: Mutex<Option<JoinHandle<()>>>,
    audio_preload_thread: Mutex<Option<JoinHandle<()>>>,
}

struct SharedPlaybackState {
    current_time_secs: Mutex<f64>,
    playing_since: Mutex<Option<PlaybackClock>>,
    /// The latest single-frame preview or the current playback frame.
    latest_frame: Mutex<Option<PlaybackFrame>>,
    /// Playback frame queue (timed + sorted by PTS).
    frame_queue: Mutex<VecDeque<TimedFrame>>,
    playback_empty_polls: AtomicU64,
    playback_drop_bursts: AtomicU64,
    last_error: Mutex<Option<String>>,
    request_generation: AtomicU64,
    video_request_in_flight: AtomicBool,
    in_playback_mode: AtomicBool,
    audio_stop_requested: AtomicBool,
    audio_loading: AtomicBool,
    audio_buffer: Mutex<Option<AudioBuffer>>,
    audio_generation: AtomicU64,
    audio_started_generation: AtomicU64,
    playback_start_target_secs: Mutex<f64>,
    frame_pool: Arc<FramePool>,
}

struct PlaybackClock {
    start_time_secs: f64,
    started_at: Option<Instant>,
}

struct AudioBuffer {
    sample_rate: u32,
    channels: u16,
    samples: Vec<i16>,
}

struct AudioSliceSource {
    samples: Vec<i16>,
    next_index: usize,
    channels: u16,
    sample_rate: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DecoderFrameKind {
    Preview,
    Playback,
}

/// Wrapper that flows through the channel.
/// On the consumer side, the inner `ColorImage` is used directly for
/// `texture.set()`. The `PooledColorImage` wrapper is consumed *here*,
/// and the backing buffer is returned to the pool on drop.
struct DecoderFrame {
    request_id: u64,
    kind: DecoderFrameKind,
    pts_secs: f64,
    image: PooledColorImage,
}

struct DecoderError {
    request_id: u64,
    message: String,
}

enum DecoderCommand {
    Preview {
        request_id: u64,
        time_secs: f64,
        fast: bool,
    },
    Playback {
        request_id: u64,
        time_secs: f64,
    },
    Stop,
    Shutdown,
}

pub struct DecodePipeline {
    command_tx: Sender<DecoderCommand>,
    frame_rx: Receiver<DecoderFrame>,
    error_rx: Receiver<DecoderError>,
    worker: Option<JoinHandle<()>>,
}

struct DecoderSession {
    video_path: PathBuf,
    input: ffmpeg::format::context::Input,
    decoder: ffmpeg::decoder::Video,
    scaler: ffmpeg::software::scaling::Context,
    decoded_frame: ffmpeg::util::frame::video::Video,
    sw_frame: ffmpeg::util::frame::video::Video,
    /// Output frame — we reuse the same frame buffer.
    rgba_frame: ffmpeg::util::frame::video::Video,
    stream_index: usize,
    output_width: u32,
    output_height: u32,
    stream_time_base_num: i32,
    stream_time_base_den: i32,
    seek_target_secs: f64,
    /// Timestamp of the frame we decoded BEFORE `seek_target_secs`.
    /// Used by the forward-decode cache to avoid redundant seeks.
    last_decoded_pts_secs: f64,
    keyframe_positions: Vec<f64>,
    keyframes_scanned: bool,
    frame_pool: Arc<FramePool>,
    hw_context: Option<DecoderHardwareContext>,
}

// ===========================================================================
// PlaybackController implementation
// ===========================================================================

impl PlaybackController {
    pub fn new(video_path: PathBuf, metadata: VideoFileMetadata, preview_width: u32) -> Self {
        let (output_width, output_height) = scaled_dimensions(preview_width, &metadata);
        let frame_pool = Arc::new(FramePool::new(output_width, output_height, FRAME_POOL_SIZE));

        let shared = Arc::new(SharedPlaybackState {
            current_time_secs: Mutex::new(0.0),
            playing_since: Mutex::new(None),
            latest_frame: Mutex::new(None),
            frame_queue: Mutex::new(VecDeque::new()),
            playback_empty_polls: AtomicU64::new(0),
            playback_drop_bursts: AtomicU64::new(0),
            last_error: Mutex::new(None),
            request_generation: AtomicU64::new(1),
            video_request_in_flight: AtomicBool::new(false),
            in_playback_mode: AtomicBool::new(false),
            audio_stop_requested: AtomicBool::new(false),
            audio_loading: AtomicBool::new(false),
            audio_buffer: Mutex::new(None),
            audio_generation: AtomicU64::new(1),
            audio_started_generation: AtomicU64::new(0),
            playback_start_target_secs: Mutex::new(0.0),
            frame_pool: frame_pool.clone(),
        });

        let decoder = DecodePipeline::new(
            video_path.clone(),
            output_width,
            output_height,
            metadata.fps.clamp(1.0, 120.0),
            frame_pool,
        );

        let (audio_shutdown_tx, audio_shutdown_rx) = std::sync::mpsc::channel();
        let (audio_handle_tx, audio_handle_rx) = std::sync::mpsc::channel();

        // Spawn thread to hold rodio OutputStream alive for the controller's lifetime.
        let audio_stream_thread = thread::spawn(move || {
            if let Ok((stream, handle)) = rodio::OutputStream::try_default() {
                let _ = audio_handle_tx.send(Some(handle));
                let _ = audio_shutdown_rx.recv();
                drop(stream);
            } else {
                let _ = audio_handle_tx.send(None);
            }
        });

        let audio_handle = audio_handle_rx.recv().unwrap_or(None);

        let controller = Self {
            metadata,
            shared,
            decoder,
            audio_handle,
            _audio_shutdown_tx: Some(audio_shutdown_tx),
            audio_stream_thread: Mutex::new(Some(audio_stream_thread)),
            audio_preload_thread: Mutex::new(None),
        };
        controller.begin_audio_preload(video_path);
        controller
    }

    fn next_request_id(&self) -> u64 {
        self.shared
            .request_generation
            .fetch_add(1, Ordering::SeqCst)
            + 1
    }

    fn active_request_id(&self) -> u64 {
        self.shared.request_generation.load(Ordering::SeqCst)
    }

    pub fn request_preview_frame(&mut self, time_secs: f64) {
        self.request_preview_at(time_secs, false);
    }

    pub fn request_preview_frame_fast(&mut self, time_secs: f64) {
        self.request_preview_at(time_secs, true);
    }

    fn request_preview_at(&mut self, time_secs: f64, fast: bool) {
        let clamped_time = self.clamp_time(time_secs);
        self.stop_audio();
        *self
            .shared
            .current_time_secs
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = clamped_time;
        *self
            .shared
            .playing_since
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = None;
        self.shared
            .frame_queue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        self.shared.frame_pool.trim_to(FRAME_POOL_IDLE_TRIM_TARGET);

        let was_playing = self.shared.in_playback_mode.swap(false, Ordering::SeqCst);
        if was_playing {
            self.decoder.stop();
        }

        let request_id = self.next_request_id();
        self.shared
            .video_request_in_flight
            .store(true, Ordering::SeqCst);
        if let Err(err) = self.decoder.request_preview(request_id, clamped_time, fast) {
            self.shared
                .video_request_in_flight
                .store(false, Ordering::SeqCst);
            *self
                .shared
                .last_error
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = Some(err.to_string());
        }
    }

    pub fn play_from(&mut self, time_secs: f64) {
        let clamped_time = self.clamp_time(time_secs);
        self.shared
            .frame_pool
            .trim_to(FRAME_POOL_ACTIVE_TRIM_TARGET);

        // Preserve the current preview frame as the first playback frame.
        let preview_frame = self
            .shared
            .latest_frame
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        {
            let mut queue = self
                .shared
                .frame_queue
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            queue.clear();
            if let Some(frame) = preview_frame {
                queue.push_back(TimedFrame {
                    pts_secs: clamped_time,
                    image: frame.image,
                });
            }
        }

        *self
            .shared
            .current_time_secs
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = clamped_time;
        *self
            .shared
            .playback_start_target_secs
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = clamped_time;
        *self
            .shared
            .playing_since
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(PlaybackClock {
            start_time_secs: clamped_time,
            started_at: None,
        });

        let request_id = self.next_request_id();
        self.shared
            .video_request_in_flight
            .store(false, Ordering::SeqCst);
        self.shared.in_playback_mode.store(true, Ordering::SeqCst);
        if let Err(err) = self.decoder.start_playback(request_id, clamped_time) {
            *self
                .shared
                .playing_since
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = None;
            *self
                .shared
                .last_error
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = Some(err.to_string());
            self.shared.in_playback_mode.store(false, Ordering::SeqCst);
            return;
        }
        self.stop_audio();
    }

    pub fn pause_at(&mut self, time_secs: f64) {
        let clamped_time = self.clamp_time(time_secs);
        *self
            .shared
            .current_time_secs
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = clamped_time;
        *self
            .shared
            .playing_since
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = None;
        self.shared
            .frame_queue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        self.shared.frame_pool.trim_to(FRAME_POOL_IDLE_TRIM_TARGET);
        self.shared
            .video_request_in_flight
            .store(false, Ordering::SeqCst);
        self.shared.in_playback_mode.store(false, Ordering::SeqCst);
        self.next_request_id();
        self.decoder.stop();
        self.stop_audio();
    }

    pub fn playback_position_secs(&self) -> f64 {
        let maybe_clock = self
            .shared
            .playing_since
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(clock) = maybe_clock.as_ref() {
            if let Some(started_at) = clock.started_at {
                return self.clamp_time(clock.start_time_secs + started_at.elapsed().as_secs_f64());
            }
            return self.clamp_time(clock.start_time_secs);
        }
        *self
            .shared
            .current_time_secs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    pub fn is_playing(&self) -> bool {
        self.shared
            .playing_since
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some()
    }

    pub fn has_pending_activity(&self) -> bool {
        self.is_playing()
            || self.shared.audio_loading.load(Ordering::SeqCst)
            || self.shared.video_request_in_flight.load(Ordering::SeqCst)
    }

    pub fn release_idle_resources(&mut self) {
        self.pause_at(self.playback_position_secs());
        let _ = self.shared.latest_frame.lock().map(|mut g| *g = None);
        let _ = self.shared.frame_queue.lock().map(|mut g| g.clear());
        let _ = self.shared.audio_buffer.lock().map(|mut g| *g = None);
        self.shared.playback_empty_polls.store(0, Ordering::SeqCst);
        self.shared.playback_drop_bursts.store(0, Ordering::SeqCst);
        self.shared.frame_pool.clear();
    }

    pub fn is_frame_request_in_flight(&self) -> bool {
        self.shared.video_request_in_flight.load(Ordering::SeqCst)
    }

    /// Take a single preview frame (from the latest_frame slot).
    pub fn take_frame(&self) -> Option<PlaybackFrame> {
        self.shared
            .latest_frame
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
    }

    /// Take a frame from the playback queue, advancing the playhead.
    ///
    /// Drops stale frames that are more than one frame interval behind
    /// the wall clock.
    pub fn take_playback_frame(&self) -> Option<ColorImage> {
        let wall_time_secs = self.playback_position_secs();
        let mut queue = self
            .shared
            .frame_queue
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        let is_buffering = {
            let clock = self
                .shared
                .playing_since
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            clock.as_ref().is_some_and(|c| c.started_at.is_none())
        };

        if queue.is_empty() {
            if is_buffering {
                self.shared.playback_empty_polls.store(0, Ordering::SeqCst);
            } else {
                let empty_polls = self
                    .shared
                    .playback_empty_polls
                    .fetch_add(1, Ordering::SeqCst)
                    + 1;
                if empty_polls == 60 || empty_polls == 180 || empty_polls.is_multiple_of(600) {
                    tracing::warn!(
                        "Playback queue empty for {} polls at wall={:.3}s",
                        empty_polls,
                        wall_time_secs
                    );
                }
            }
            return None;
        }

        let frame_duration = 1.0 / self.metadata.fps.max(1.0);
        let mut dropped_count = 0u32;

        while queue.len() > 1 {
            if let Some(front) = queue.front() {
                if wall_time_secs - front.pts_secs > frame_duration {
                    queue.pop_front();
                    dropped_count += 1;
                } else {
                    break;
                }
            } else {
                break;
            }
        }

        if dropped_count >= 4 {
            let burst = self
                .shared
                .playback_drop_bursts
                .fetch_add(1, Ordering::SeqCst)
                + 1;
            if burst <= 3 || burst.is_multiple_of(30) {
                tracing::warn!(
                    "Playback dropped {} stale frame(s) at wall={:.3}s (burst #{})",
                    dropped_count,
                    wall_time_secs,
                    burst
                );
            }
            let quality_sample = GalleryPlaybackRuntimeSample {
                stale_frames_dropped: dropped_count,
                empty_queue_polls: self.shared.playback_empty_polls.load(Ordering::SeqCst),
                queue_depth: queue.len(),
            };
            let quality_assessment = assess_gallery_playback_runtime(quality_sample);
            if !quality_assessment.within_contract {
                tracing::warn!(
                    "Gallery playback quality contract exceeded: stale_dropped={} (limit {}), \
                     empty_polls={} (limit {}), queue_depth={} (limit {})",
                    quality_sample.stale_frames_dropped,
                    GALLERY_PLAYBACK_GUARDRAIL.max_stale_frames_dropped_per_poll,
                    quality_sample.empty_queue_polls,
                    GALLERY_PLAYBACK_GUARDRAIL.max_empty_queue_polls,
                    quality_sample.queue_depth,
                    GALLERY_PLAYBACK_GUARDRAIL.max_queue_depth_frames
                );
            }
        }

        if queue.is_empty() {
            return None;
        }

        // Find all frames with pts <= wall_time and return the last (closest).
        let mut frames_to_remove = 0;
        for (idx, frame) in queue.iter().enumerate() {
            if frame.pts_secs <= wall_time_secs {
                frames_to_remove = idx + 1;
            } else {
                break;
            }
        }

        if frames_to_remove > 0 {
            for _ in 0..frames_to_remove - 1 {
                queue.pop_front();
            }
            let frame = queue.pop_front()?;
            self.shared.playback_empty_polls.store(0, Ordering::SeqCst);
            return Some(frame.image);
        }

        // No frame with pts <= wall_time. If the first frame is within one
        // frame duration ahead, show it early (smooth playback).
        if let Some(front) = queue.front() {
            let ahead_by = front.pts_secs - wall_time_secs;
            if ahead_by <= frame_duration {
                let frame = queue.pop_front()?;
                self.shared.playback_empty_polls.store(0, Ordering::SeqCst);
                return Some(frame.image);
            }
        }
        None
    }

    pub fn take_error(&self) -> Option<String> {
        self.shared
            .last_error
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
    }

    pub fn playback_fps(&self) -> f64 {
        self.metadata.fps.clamp(1.0, 120.0)
    }

    pub fn poll(&mut self) {
        let queue_depth_limit = if self.is_playing() {
            PLAYBACK_QUEUE_DEPTH_PLAYING
        } else {
            PLAYBACK_QUEUE_DEPTH_PAUSED
        };
        let queue_is_saturated = if self.is_playing() {
            let queue_len = self
                .shared
                .frame_queue
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .len();
            queue_len >= queue_depth_limit
        } else {
            false
        };

        if !queue_is_saturated {
            while let Some(frame) = self.decoder.try_recv_frame() {
                let active_request = self.active_request_id();
                if frame.request_id != active_request {
                    tracing::trace!(
                        "poll: discarding {0:?} frame request_id={1} != active={2}",
                        frame.kind,
                        frame.request_id,
                        active_request
                    );
                    continue;
                }

                match frame.kind {
                    DecoderFrameKind::Preview => {
                        // Convert PooledColorImage → PlaybackFrame by taking the ColorImage.
                        // The pool buffer is consumed here (passed to texture.set eventually).
                        let image = frame.image.into_color_image();
                        *self
                            .shared
                            .latest_frame
                            .lock()
                            .unwrap_or_else(|e| e.into_inner()) = Some(PlaybackFrame {
                            image,
                            pts_secs: frame.pts_secs,
                        });
                        self.shared
                            .video_request_in_flight
                            .store(false, Ordering::SeqCst);
                    }
                    DecoderFrameKind::Playback => {
                        let mut queue = self
                            .shared
                            .frame_queue
                            .lock()
                            .unwrap_or_else(|e| e.into_inner());
                        let pts = frame.pts_secs;
                        if queue.len() >= queue_depth_limit {
                            break;
                        }
                        // Consume PooledColorImage → TimedFrame.
                        let image = frame.image.into_color_image();
                        queue.push_back(TimedFrame {
                            pts_secs: pts,
                            image,
                        });
                    }
                }
            }
        }

        while let Some(error) = self.decoder.try_recv_error() {
            if error.request_id != 0 && error.request_id != self.active_request_id() {
                continue;
            }
            self.shared
                .video_request_in_flight
                .store(false, Ordering::SeqCst);
            *self
                .shared
                .last_error
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = Some(error.message);
        }

        if !self.is_playing() {
            return;
        }

        self.check_and_start_clock();

        let current_time = self.playback_position_secs();
        *self
            .shared
            .current_time_secs
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = current_time;
        if current_time >= self.metadata.duration_secs {
            self.pause_at(self.metadata.duration_secs);
        }
    }

    fn check_and_start_clock(&mut self) {
        let mut clock_unlocked = false;
        {
            let mut maybe_clock = self
                .shared
                .playing_since
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(clock) = maybe_clock.as_mut() {
                if clock.started_at.is_none() {
                    let queue = self
                        .shared
                        .frame_queue
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    if queue.front().is_some() {
                        let target_start = *self
                            .shared
                            .playback_start_target_secs
                            .lock()
                            .unwrap_or_else(|e| e.into_inner());
                        clock.start_time_secs = target_start;
                        clock.started_at = Some(Instant::now());
                        clock_unlocked = true;
                    }
                }
            }
        }
        if clock_unlocked {
            self.maybe_start_audio();
        }
    }

    pub fn cache_stats(&self) -> (usize, f64) {
        let queue = self
            .shared
            .frame_queue
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let count = queue.len();
        let mb = queue
            .iter()
            .map(|f| f.image.pixels.len() * 4)
            .sum::<usize>() as f64
            / (1024.0 * 1024.0);
        (count, mb)
    }

    pub fn cached_frame_count(&self) -> usize {
        self.shared
            .frame_queue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len()
    }

    fn clamp_time(&self, time_secs: f64) -> f64 {
        time_secs.clamp(0.0, self.metadata.duration_secs)
    }

    fn begin_audio_preload(&self, video_path: PathBuf) {
        if !self.metadata.has_audio || self.shared.audio_loading.swap(true, Ordering::SeqCst) {
            return;
        }

        let shared = self.shared.clone();
        let handle = thread::spawn(move || {
            let result = decode_audio_track(&video_path);
            match result {
                Ok(buffer) => {
                    *shared
                        .audio_buffer
                        .lock()
                        .unwrap_or_else(|e| e.into_inner()) = Some(buffer);
                }
                Err(err) => {
                    *shared.last_error.lock().unwrap_or_else(|e| e.into_inner()) =
                        Some(format!("Audio preload failed: {err:#}"));
                }
            }
            shared.audio_loading.store(false, Ordering::SeqCst);
        });
        if let Ok(mut guard) = self.audio_preload_thread.lock() {
            *guard = Some(handle);
        }
    }

    fn maybe_start_audio(&mut self) {
        let Some(buffer) = self
            .shared
            .audio_buffer
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(clone_audio_buffer)
        else {
            return;
        };

        let current_audio_generation = self.shared.audio_generation.load(Ordering::SeqCst);
        if self.shared.audio_started_generation.load(Ordering::SeqCst) == current_audio_generation {
            return;
        }

        let Some(handle) = self.audio_handle.clone() else {
            return;
        };

        let start_time = self.playback_position_secs();
        let generation = self.shared.audio_generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.shared
            .audio_started_generation
            .store(generation, Ordering::SeqCst);
        self.shared
            .audio_stop_requested
            .store(false, Ordering::SeqCst);
        let shared = self.shared.clone();
        let _ = thread::spawn(move || {
            let sink = match Sink::try_new(&handle) {
                Ok(sink) => sink,
                Err(err) => {
                    *shared.last_error.lock().unwrap_or_else(|e| e.into_inner()) =
                        Some(format!("Audio output failed: {err}"));
                    return;
                }
            };
            sink.append(AudioSliceSource::new(buffer, start_time));
            loop {
                if shared.audio_stop_requested.load(Ordering::SeqCst)
                    || shared.audio_generation.load(Ordering::SeqCst) != generation
                {
                    sink.stop();
                    break;
                }
                if sink.empty() {
                    break;
                }
                thread::sleep(Duration::from_millis(50));
            }
            drop(sink);
        });
    }

    fn stop_audio(&mut self) {
        self.shared.audio_generation.fetch_add(1, Ordering::SeqCst);
        self.shared
            .audio_stop_requested
            .store(true, Ordering::SeqCst);
    }
}

impl Drop for PlaybackController {
    fn drop(&mut self) {
        self.shared
            .playing_since
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        self.shared.audio_generation.fetch_add(1, Ordering::SeqCst);
        self.shared
            .audio_stop_requested
            .store(true, Ordering::SeqCst);

        let _ = self.shared.audio_buffer.lock().map(|mut g| *g = None);

        self.decoder.stop();
        let _ = self.shared.frame_queue.lock().map(|mut g| g.clear());
        let _ = self.shared.latest_frame.lock().map(|mut g| *g = None);

        self._audio_shutdown_tx.take();

        if let Ok(mut guard) = self.audio_preload_thread.lock() {
            if let Some(handle) = guard.take() {
                let _ = handle.join();
            }
        }
        if let Ok(mut guard) = self.audio_stream_thread.lock() {
            if let Some(handle) = guard.take() {
                let _ = handle.join();
            }
        }

        self.shared.frame_pool.clear();
    }
}

// ===========================================================================
// DecodePipeline implementation
// ===========================================================================

impl DecodePipeline {
    fn new(
        video_path: PathBuf,
        output_width: u32,
        output_height: u32,
        fps: f64,
        frame_pool: Arc<FramePool>,
    ) -> Self {
        let (command_tx, command_rx) = bounded(16);
        let (frame_tx, frame_rx) = bounded(FRAME_CHANNEL_CAPACITY);
        let (error_tx, error_rx) = bounded(16);
        let worker = thread::spawn(move || {
            decoder_worker_loop(
                video_path,
                output_width,
                output_height,
                fps,
                command_rx,
                frame_tx,
                error_tx,
                frame_pool,
            );
        });
        Self {
            command_tx,
            frame_rx,
            error_rx,
            worker: Some(worker),
        }
    }

    fn request_preview(&self, request_id: u64, time_secs: f64, fast: bool) -> Result<()> {
        self.command_tx
            .send(DecoderCommand::Preview {
                request_id,
                time_secs,
                fast,
            })
            .map_err(|_| anyhow!("Decoder worker is unavailable"))
    }

    fn start_playback(&self, request_id: u64, time_secs: f64) -> Result<()> {
        self.command_tx
            .send(DecoderCommand::Playback {
                request_id,
                time_secs,
            })
            .map_err(|_| anyhow!("Decoder worker is unavailable"))
    }

    fn stop(&self) {
        let _ = self.command_tx.send(DecoderCommand::Stop);
    }

    fn try_recv_frame(&self) -> Option<DecoderFrame> {
        match self.frame_rx.try_recv() {
            Ok(frame) => Some(frame),
            Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => None,
        }
    }

    fn try_recv_error(&self) -> Option<DecoderError> {
        match self.error_rx.try_recv() {
            Ok(error) => Some(error),
            Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => None,
        }
    }
}

impl Drop for DecodePipeline {
    fn drop(&mut self) {
        let _ = self.command_tx.send(DecoderCommand::Shutdown);
        while self.frame_rx.try_recv().is_ok() {}
        while self.error_rx.try_recv().is_ok() {}
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

// ===========================================================================
// Channel helper: send frame or handle incoming command
// ===========================================================================

enum SendFrameOutcome {
    Sent,
    CommandArrived {
        command: DecoderCommand,
        unsent_frame: DecoderFrame,
    },
}

fn send_playback_frame(
    command_rx: &Receiver<DecoderCommand>,
    frame_tx: &Sender<DecoderFrame>,
    frame: DecoderFrame,
) -> Result<SendFrameOutcome> {
    crossbeam_channel::select! {
        send(frame_tx, frame) -> send_result => {
            match send_result {
                Ok(()) => Ok(SendFrameOutcome::Sent),
                Err(crossbeam_channel::SendError(_)) => {
                    bail!("Playback frame channel disconnected");
                }
            }
        }
        recv(command_rx) -> command_result => {
            match command_result {
                Ok(command @ DecoderCommand::Shutdown)
                | Ok(command @ DecoderCommand::Stop)
                | Ok(command @ DecoderCommand::Preview { .. })
                | Ok(command @ DecoderCommand::Playback { .. }) => {
                    Ok(SendFrameOutcome::CommandArrived { command, unsent_frame: frame })
                }
                Err(_) => {
                    bail!("Decoder command channel disconnected");
                }
            }
        }
    }
}

// ===========================================================================
// Decoder worker loop
// ===========================================================================

#[allow(clippy::too_many_arguments)]
fn decoder_worker_loop(
    video_path: PathBuf,
    output_width: u32,
    output_height: u32,
    fps: f64,
    command_rx: Receiver<DecoderCommand>,
    frame_tx: Sender<DecoderFrame>,
    error_tx: Sender<DecoderError>,
    frame_pool: Arc<FramePool>,
) {
    let _ = ffmpeg::init();
    let mut session =
        match DecoderSession::open(&video_path, output_width, output_height, fps, frame_pool) {
            Ok(session) => session,
            Err(err) => {
                let _ = error_tx.send(DecoderError {
                    request_id: 0,
                    message: format!("Failed to initialize video decoder: {err:#}"),
                });
                return;
            }
        };

    // Proactively scan keyframes at startup so the first preview request
    // (typically at time=0) doesn't block on a full-file sequential read.
    // Without this, seek_to() would call scan_keyframes() synchronously,
    // blocking the decoder worker for many seconds on large files and
    // causing the preview request to go stale (request_generation bumped
    // by pause_at() when the user scrubs, leading to the frame being
    // discarded by the request_id check in poll()).
    tracing::info!(
        "Keyframe scan complete ({} positions), entering command loop",
        session.keyframe_positions.len()
    );

    loop {
        let command = match command_rx.recv() {
            Ok(command) => command,
            Err(_) => {
                tracing::debug!("Decoder command channel disconnected, exiting");
                return;
            }
        };

        tracing::trace!(
            "Decoder received command: {:?}",
            std::mem::discriminant(&command)
        );

        match command {
            DecoderCommand::Shutdown => {
                session.decoder.send_eof().ok();
                unsafe {
                    ffmpeg::ffi::avcodec_flush_buffers(session.decoder.as_mut_ptr());
                }
                drop(session);
                return;
            }
            DecoderCommand::Stop => {
                continue;
            }
            DecoderCommand::Preview {
                request_id,
                time_secs,
                fast,
            } => {
                let skip_mode = if fast {
                    ffmpeg::ffi::AVDiscard::AVDISCARD_NONINTRA
                } else {
                    ffmpeg::ffi::AVDiscard::AVDISCARD_NONREF
                };
                if let Err(err) = session.set_skip_frame_mode(skip_mode) {
                    let _ = error_tx.send(DecoderError {
                        request_id,
                        message: format!("Failed to set preview skip frame mode: {err:#}"),
                    });
                    continue;
                }

                // --- Phase 3: forward-decode cache ---
                // If the new target is ahead of the last decoded PTS and within
                // the forward-decode threshold, decode forward instead of seeking.
                // This avoids keyframe-seek + frame-skip latency during short scrubs.
                let is_forward = !fast
                    && time_secs > session.last_decoded_pts_secs
                    && (time_secs - session.last_decoded_pts_secs) <= FORWARD_DECODE_THRESHOLD_SECS;

                if is_forward {
                    tracing::trace!(
                        "Preview request {request_id}: forward-decode from {:.3}s → {:.3}s \
                         (delta={:.3}s)",
                        session.last_decoded_pts_secs,
                        time_secs,
                        time_secs - session.last_decoded_pts_secs
                    );
                    // Decode forward until we reach the target time.
                    loop {
                        match session.decode_next_image() {
                            Ok(Some((pts_secs, image))) => {
                                if pts_secs >= time_secs {
                                    let _ = frame_tx.send(DecoderFrame {
                                        request_id,
                                        kind: DecoderFrameKind::Preview,
                                        pts_secs,
                                        image,
                                    });
                                    break;
                                }
                                // Continue decoding — this frame is before the target.
                            }
                            Ok(None) => {
                                let _ = error_tx.send(DecoderError {
                                    request_id,
                                    message: "Forward decode: no more frames".to_string(),
                                });
                                break;
                            }
                            Err(err) => {
                                let _ = error_tx.send(DecoderError {
                                    request_id,
                                    message: format!("Forward decode failed: {err:#}"),
                                });
                                break;
                            }
                        }
                    }
                } else {
                    // Standard seek + decode.
                    tracing::trace!(
                        "Preview request {request_id}: seek+decode to {:.3}s",
                        time_secs
                    );
                    if let Err(err) = session.seek_to(time_secs) {
                        tracing::warn!(
                            "Preview request {request_id}: seek_to({:.3}s) failed: {err:#}",
                            time_secs
                        );
                        let _ = error_tx.send(DecoderError {
                            request_id,
                            message: format!("Preview seek failed: {err:#}"),
                        });
                        continue;
                    }

                    match session.decode_next_image() {
                        Ok(Some((pts_secs, image))) => {
                            tracing::trace!(
                                "Preview request {request_id}: decoded frame at {:.3}s",
                                pts_secs
                            );
                            let _ = frame_tx.send(DecoderFrame {
                                request_id,
                                kind: DecoderFrameKind::Preview,
                                pts_secs,
                                image,
                            });
                        }
                        Ok(None) => {
                            let _ = error_tx.send(DecoderError {
                                request_id,
                                message: "No preview frame could be decoded".to_string(),
                            });
                        }
                        Err(err) => {
                            let _ = error_tx.send(DecoderError {
                                request_id,
                                message: format!("Preview decode failed: {err:#}"),
                            });
                        }
                    }
                }
            }
            DecoderCommand::Playback {
                request_id,
                time_secs,
            } => {
                if let Err(err) =
                    session.set_skip_frame_mode(ffmpeg::ffi::AVDiscard::AVDISCARD_DEFAULT)
                {
                    let _ = error_tx.send(DecoderError {
                        request_id,
                        message: format!("Failed to set playback skip frame mode: {err:#}"),
                    });
                    continue;
                }
                if let Err(err) = session.seek_to(time_secs) {
                    let _ = error_tx.send(DecoderError {
                        request_id,
                        message: format!("Playback seek failed: {err:#}"),
                    });
                    continue;
                }

                let mut active_request_id = request_id;
                'playback: loop {
                    match command_rx.try_recv() {
                        Ok(DecoderCommand::Shutdown) => return,
                        Ok(DecoderCommand::Stop) => break 'playback,
                        Ok(DecoderCommand::Preview {
                            request_id,
                            time_secs,
                            fast,
                        }) => {
                            let skip_mode = if fast {
                                ffmpeg::ffi::AVDiscard::AVDISCARD_NONINTRA
                            } else {
                                ffmpeg::ffi::AVDiscard::AVDISCARD_NONREF
                            };
                            let _ = session.set_skip_frame_mode(skip_mode);
                            if let Err(err) = session.seek_to(time_secs) {
                                let _ = error_tx.send(DecoderError {
                                    request_id,
                                    message: format!("Preview seek failed: {err:#}"),
                                });
                            } else {
                                if let Ok(Some((pts_secs, image))) = session.decode_next_image() {
                                    let _ = frame_tx.send(DecoderFrame {
                                        request_id,
                                        kind: DecoderFrameKind::Preview,
                                        pts_secs,
                                        image,
                                    });
                                }
                            }
                            break 'playback;
                        }
                        Ok(DecoderCommand::Playback {
                            request_id,
                            time_secs,
                        }) => {
                            active_request_id = request_id;
                            let _ = session
                                .set_skip_frame_mode(ffmpeg::ffi::AVDiscard::AVDISCARD_DEFAULT);
                            let _ = session.seek_to(time_secs);
                        }
                        Err(TryRecvError::Disconnected) => return,
                        Err(TryRecvError::Empty) => {}
                    }

                    match session.decode_next_image() {
                        Ok(Some((pts_secs, image))) => {
                            let frame = DecoderFrame {
                                request_id: active_request_id,
                                kind: DecoderFrameKind::Playback,
                                pts_secs,
                                image,
                            };
                            match send_playback_frame(&command_rx, &frame_tx, frame) {
                                Ok(SendFrameOutcome::Sent) => {}
                                Ok(SendFrameOutcome::CommandArrived {
                                    command,
                                    unsent_frame,
                                }) => match command {
                                    DecoderCommand::Shutdown => return,
                                    DecoderCommand::Stop => break 'playback,
                                    DecoderCommand::Preview {
                                        request_id,
                                        time_secs,
                                        fast,
                                    } => {
                                        let skip_mode = if fast {
                                            ffmpeg::ffi::AVDiscard::AVDISCARD_NONINTRA
                                        } else {
                                            ffmpeg::ffi::AVDiscard::AVDISCARD_NONREF
                                        };
                                        let _ = session.set_skip_frame_mode(skip_mode);
                                        let _ = session.seek_to(time_secs);
                                        if let Ok(Some((pts_secs, image))) =
                                            session.decode_next_image()
                                        {
                                            let _ = frame_tx.send(DecoderFrame {
                                                request_id,
                                                kind: DecoderFrameKind::Preview,
                                                pts_secs,
                                                image,
                                            });
                                        }
                                        break 'playback;
                                    }
                                    DecoderCommand::Playback {
                                        request_id,
                                        time_secs,
                                    } => {
                                        let _ = frame_tx.send(unsent_frame);
                                        active_request_id = request_id;
                                        let _ = session.seek_to(time_secs);
                                    }
                                },
                                Err(err) => {
                                    let _ = error_tx.send(DecoderError {
                                        request_id: active_request_id,
                                        message: format!("Playback decode failed: {err:#}"),
                                    });
                                    break 'playback;
                                }
                            }
                        }
                        Ok(None) => break 'playback,
                        Err(err) => {
                            let _ = error_tx.send(DecoderError {
                                request_id: active_request_id,
                                message: format!("Playback decode failed: {err:#}"),
                            });
                            break 'playback;
                        }
                    }
                }
            }
        }
    }
}

// ===========================================================================
// DecoderSession — the actual FFmpeg decoder logic
// ===========================================================================

impl DecoderSession {
    fn open(
        video_path: &Path,
        output_width: u32,
        output_height: u32,
        _fps: f64,
        frame_pool: Arc<FramePool>,
    ) -> Result<Self> {
        let input = ffmpeg::format::input(video_path)
            .with_context(|| format!("Failed to open video file: {video_path:?}"))?;
        let input_stream = input
            .streams()
            .best(ffmpeg::media::Type::Video)
            .context("No video stream found")?;
        let stream_index = input_stream.index();
        let stream_time_base = input_stream.time_base();

        let mut hw_context = Self::create_decoder_hardware_context().ok();
        let (decoder, using_hw) = match Self::open_video_decoder(&input_stream, hw_context.as_ref())
        {
            Ok(decoder) => (decoder, hw_context.is_some()),
            Err(err) => {
                if hw_context.is_some() {
                    tracing::warn!(
                        "Hardware decode unavailable, falling back to software: {err:#}"
                    );
                }
                hw_context = None;
                (Self::open_video_decoder(&input_stream, None)?, false)
            }
        };

        let input_format = decoder.format();
        let input_width = decoder.width();
        let input_height = decoder.height();

        let sw_format = if using_hw {
            ffmpeg::format::Pixel::NV12
        } else {
            input_format
        };

        // Normal scaler (BILINEAR — good quality).
        let scaler = ffmpeg::software::scaling::Context::get(
            sw_format,
            input_width,
            input_height,
            ffmpeg::format::Pixel::RGBA,
            output_width,
            output_height,
            ffmpeg::software::scaling::flag::Flags::BILINEAR,
        )
        .context("Failed to create decoder scaler")?;

        let session = Self {
            video_path: video_path.to_path_buf(),
            input,
            decoder,
            scaler,
            decoded_frame: ffmpeg::util::frame::video::Video::empty(),
            sw_frame: if using_hw {
                ffmpeg::util::frame::video::Video::new(
                    ffmpeg::format::Pixel::NV12,
                    input_width,
                    input_height,
                )
            } else {
                ffmpeg::util::frame::video::Video::empty()
            },
            rgba_frame: ffmpeg::util::frame::video::Video::new(
                ffmpeg::format::Pixel::RGBA,
                output_width,
                output_height,
            ),
            stream_index,
            output_width,
            output_height,
            stream_time_base_num: stream_time_base.0,
            stream_time_base_den: stream_time_base.1,
            seek_target_secs: 0.0,
            last_decoded_pts_secs: 0.0,
            keyframe_positions: Vec::new(),
            keyframes_scanned: false,
            frame_pool,
            hw_context,
        };

        if using_hw {
            tracing::info!("Using hardware decoding (D3D11VA)");
        }

        Ok(session)
    }

    fn create_decoder_hardware_context() -> Result<DecoderHardwareContext> {
        unsafe {
            let mut device_ctx_ref = std::ptr::null_mut();
            let result = ffmpeg::ffi::av_hwdevice_ctx_create(
                &mut device_ctx_ref,
                ffmpeg::ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_D3D11VA,
                std::ptr::null(),
                std::ptr::null_mut(),
                0,
            );
            if result < 0 {
                bail!("Failed to create D3D11VA hardware device: {}", result);
            }
            Ok(DecoderHardwareContext { device_ctx_ref })
        }
    }

    fn open_video_decoder(
        input_stream: &ffmpeg::format::stream::Stream<'_>,
        hw_context: Option<&DecoderHardwareContext>,
    ) -> Result<ffmpeg::decoder::Video> {
        let mut context =
            ffmpeg::codec::context::Context::from_parameters(input_stream.parameters())
                .context("Failed to create decoder context")?;

        let mut hw_device_ctx_ref = std::ptr::null_mut();
        if let Some(hw_context) = hw_context {
            unsafe {
                hw_device_ctx_ref = ffmpeg::ffi::av_buffer_ref(hw_context.device_ctx_ref);
                if hw_device_ctx_ref.is_null() {
                    bail!("Failed to reference D3D11VA hardware device");
                }
                let codec_ctx = context.as_mut_ptr();
                (*codec_ctx).hw_device_ctx = hw_device_ctx_ref;
                (*codec_ctx).get_format = Some(Self::select_decoder_format);
                (*codec_ctx).hwaccel_flags |= ffmpeg::ffi::AV_HWACCEL_FLAG_IGNORE_LEVEL;
            }
        }

        let decoder = match context.decoder().video() {
            Ok(decoder) => decoder,
            Err(err) => {
                if !hw_device_ctx_ref.is_null() {
                    unsafe {
                        ffmpeg::ffi::av_buffer_unref(&mut hw_device_ctx_ref);
                    }
                }
                return Err(anyhow!(err).context("Failed to open video decoder"));
            }
        };
        Ok(decoder)
    }

    unsafe extern "C" fn select_decoder_format(
        _ctx: *mut ffmpeg::ffi::AVCodecContext,
        pix_fmts: *const ffmpeg::ffi::AVPixelFormat,
    ) -> ffmpeg::ffi::AVPixelFormat {
        let mut fmt = pix_fmts;
        while !fmt.is_null() && *fmt != ffmpeg::ffi::AVPixelFormat::AV_PIX_FMT_NONE {
            if *fmt == ffmpeg::format::Pixel::D3D11.into() {
                return *fmt;
            }
            fmt = fmt.add(1);
        }
        if pix_fmts.is_null() {
            ffmpeg::ffi::AVPixelFormat::AV_PIX_FMT_NONE
        } else {
            *pix_fmts
        }
    }

    fn scan_keyframes(&mut self) {
        let loaded = self.try_load_cached_keyframes();

        if loaded {
            tracing::info!("Loaded {} cached keyframes", self.keyframe_positions.len());
            self.keyframes_scanned = true;
        } else {
            let start = Instant::now();
            let mut keyframe_pts: Vec<i64> = Vec::new();

            loop {
                let mut packet = ffmpeg::Packet::empty();
                match packet.read(&mut self.input) {
                    Ok(()) => {
                        if packet.stream() == self.stream_index {
                            let flags = unsafe { (*packet.as_mut_ptr()).flags };
                            if flags & ffmpeg::ffi::AV_PKT_FLAG_KEY != 0 {
                                let pts = unsafe { (*packet.as_mut_ptr()).pts };
                                if pts != ffmpeg::ffi::AV_NOPTS_VALUE {
                                    keyframe_pts.push(pts);
                                }
                            }
                        }
                    }
                    Err(ffmpeg::Error::Eof) => break,
                    Err(_) => break,
                }
            }

            let time_base = self.stream_time_base_num as f64 / self.stream_time_base_den as f64;
            let mut positions: Vec<f64> = keyframe_pts
                .into_iter()
                .map(|pts| (pts as f64 * time_base).max(0.0))
                .filter(|&t| t.is_finite())
                .collect();

            positions.sort_by(|a, b| a.total_cmp(b));
            positions.dedup_by(|a, b| (*a - *b).abs() < 0.01);

            // Thin to max 500 positions to keep the index compact.
            if positions.len() > 500 {
                let step = positions.len() as f64 / 500.0;
                positions = positions
                    .into_iter()
                    .enumerate()
                    .filter(|(i, _)| (*i as f64 / step).fract() < 0.5)
                    .map(|(_, v)| v)
                    .collect();
            }

            self.keyframe_positions = positions;
            self.keyframes_scanned = true;

            tracing::info!(
                "Scanned {} keyframes in {:.1}ms",
                self.keyframe_positions.len(),
                start.elapsed().as_secs_f64() * 1000.0
            );

            self.save_keyframes_cache();
        }

        // NOTE: We do NOT seek here. The input is left at EOF (after reading
        // all packets). seek_to() handles positioning via avformat_flush +
        // av_seek_frame when the first Preview/Playback command arrives.
        // Avoiding a redundant seek-to-0 here prevents potential demuxer
        // confusion from double-seeking to the same position.
    }

    fn get_keyframe_cache_path(&self) -> PathBuf {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.video_path.hash(&mut hasher);
        if let Ok(metadata) = std::fs::metadata(&self.video_path) {
            if let Ok(modified) = metadata.modified() {
                if let Ok(duration) = modified.elapsed() {
                    duration.as_secs().hash(&mut hasher);
                }
            }
        }
        let cache_dir = self
            .video_path
            .parent()
            .map(|p| p.join(".liteclip_cache"))
            .unwrap_or_else(std::env::temp_dir);
        cache_dir.join(format!("kf_{:016x}.bin", hasher.finish()))
    }

    fn try_load_cached_keyframes(&mut self) -> bool {
        let cache_path = self.get_keyframe_cache_path();
        if !cache_path.exists() {
            return false;
        }
        match std::fs::read(&cache_path) {
            Ok(data) => {
                if data.len() < 8 || &data[0..4] != b"KFC1" {
                    return false;
                }
                let count = u32::from_le_bytes([data[4], data[5], data[6], data[7]]) as usize;
                if data.len() != 8 + count * 8 {
                    return false;
                }
                let mut positions = Vec::with_capacity(count);
                for i in 0..count {
                    let offset = 8 + i * 8;
                    let val = f64::from_le_bytes([
                        data[offset],
                        data[offset + 1],
                        data[offset + 2],
                        data[offset + 3],
                        data[offset + 4],
                        data[offset + 5],
                        data[offset + 6],
                        data[offset + 7],
                    ]);
                    positions.push(val);
                }
                self.keyframe_positions = positions;
                true
            }
            Err(_) => false,
        }
    }

    fn save_keyframes_cache(&self) {
        if self.keyframe_positions.is_empty() {
            return;
        }
        let cache_path = self.get_keyframe_cache_path();
        if let Some(parent) = cache_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let count = self.keyframe_positions.len() as u32;
        let mut data = Vec::with_capacity(8 + count as usize * 8);
        data.extend_from_slice(b"KFC1");
        data.extend_from_slice(&count.to_le_bytes());
        for &pos in &self.keyframe_positions {
            data.extend_from_slice(&pos.to_le_bytes());
        }
        let _ = std::fs::write(&cache_path, &data);
    }

    fn seek_to(&mut self, time_secs: f64) -> Result<()> {
        // Keyframes should already be scanned at startup (decoder_worker_loop),
        // but fall through gracefully if not (defensive).
        if !self.keyframes_scanned {
            self.scan_keyframes();
        }

        let seek_time = if self.keyframes_scanned && !self.keyframe_positions.is_empty() {
            let idx = self
                .keyframe_positions
                .partition_point(|&k| k <= time_secs)
                .saturating_sub(1);

            match self.keyframe_positions.get(idx) {
                Some(&kf) => {
                    tracing::trace!(
                        "Seek to {:.3}s: using keyframe at {:.3}s (idx {}/{})",
                        time_secs,
                        kf,
                        idx,
                        self.keyframe_positions.len()
                    );
                    // Use the nearest keyframe AT or BEFORE the target.
                    // For time_secs=0 this picks the first keyframe, avoiding
                    // seeks to timestamp 0 which can fail on files with edit
                    // lists ("Missing key frame while searching for timestamp: 0").
                    kf.max(0.001)
                }
                None => time_secs,
            }
        } else {
            time_secs
        };

        // Add a small epsilon (1 ms) to avoid exact-timestamp-0 seeks that
        // fail on some MP4 files with edit lists (no keyframe at time 0).
        let timestamp =
            ((seek_time.max(0.0) + 0.001) * ffmpeg::ffi::AV_TIME_BASE as f64).round() as i64;
        // Use avformat_seek_file (more robust than av_seek_frame) so the
        // demuxer can freely pick the right stream/timebase for seeking.
        let result = unsafe {
            ffmpeg::ffi::avformat_seek_file(
                self.input.as_mut_ptr(),
                -1,
                i64::MIN,
                timestamp,
                i64::MAX,
                ffmpeg::ffi::AVSEEK_FLAG_BACKWARD,
            )
        };
        if result < 0 {
            // If seek fails, try one more time with AVSEEK_FLAG_ANY.
            // This allows seeking to NON-keyframe positions, which is
            // needed on some files where no keyframe exists at the
            // exact target time (e.g., edit lists shifting PTS).
            let result = unsafe {
                ffmpeg::ffi::avformat_seek_file(
                    self.input.as_mut_ptr(),
                    -1,
                    i64::MIN,
                    timestamp,
                    i64::MAX,
                    ffmpeg::ffi::AVSEEK_FLAG_BACKWARD | ffmpeg::ffi::AVSEEK_FLAG_ANY,
                )
            };
            if result < 0 {
                bail!(
                    "seek to {:.3}s failed with error code {}",
                    time_secs,
                    result
                );
            }
        }
        unsafe {
            ffmpeg::ffi::avformat_flush(self.input.as_mut_ptr());
            ffmpeg::ffi::avcodec_flush_buffers(self.decoder.as_mut_ptr());
        }
        self.seek_target_secs = time_secs.max(0.0);
        self.last_decoded_pts_secs = self.seek_target_secs;
        Ok(())
    }

    fn decode_next_image(&mut self) -> Result<Option<(f64, PooledColorImage)>> {
        loop {
            match self.decoder.receive_frame(&mut self.decoded_frame) {
                Ok(()) => {
                    let pts_secs = self.frame_pts_secs();

                    if pts_secs + 0.001 < self.seek_target_secs {
                        continue;
                    }

                    // Use fast scaler (NEAREST) during I-frame-only scrubbing,
                    // normal scaler (BILINEAR) otherwise.
                    if self.hw_context.is_some()
                        && self.decoded_frame.format() == ffmpeg::format::Pixel::D3D11
                    {
                        self.transfer_hw_frame_to_cpu()?;
                        self.scaler
                            .run(&self.sw_frame, &mut self.rgba_frame)
                            .context("Failed to scale hardware frame")?;
                    } else {
                        self.scaler
                            .run(&self.decoded_frame, &mut self.rgba_frame)
                            .context("Failed to scale decoded frame")?;
                    }

                    let image = rgba_frame_to_color_image(
                        &self.rgba_frame,
                        self.output_width,
                        self.output_height,
                        &self.frame_pool,
                    )?;
                    self.last_decoded_pts_secs = pts_secs;
                    return Ok(Some((pts_secs, image)));
                }
                Err(ffmpeg::Error::Other { errno }) if errno == ffmpeg::util::error::EAGAIN => {}
                Err(ffmpeg::Error::Eof) => return Ok(None),
                Err(err) => return Err(anyhow!(err).context("Failed receiving decoded frame")),
            }

            let mut packet = ffmpeg::Packet::empty();
            loop {
                match packet.read(&mut self.input) {
                    Ok(()) => {
                        if packet.stream() != self.stream_index {
                            continue;
                        }
                        self.decoder
                            .send_packet(&packet)
                            .context("Failed sending packet to decoder")?;
                        break;
                    }
                    Err(ffmpeg::Error::Eof) => {
                        self.decoder.send_eof().ok();
                        break;
                    }
                    Err(err) => {
                        return Err(anyhow!(err).context("Failed reading video packet"));
                    }
                }
            }
        }
    }

    fn set_skip_frame_mode(&mut self, skip_mode: ffmpeg::ffi::AVDiscard) -> Result<()> {
        unsafe {
            let codec_ctx = self.decoder.as_mut_ptr();
            if codec_ctx.is_null() {
                bail!("Codec context is null");
            }
            (*codec_ctx).skip_frame = skip_mode;
        }
        Ok(())
    }

    fn transfer_hw_frame_to_cpu(&mut self) -> Result<()> {
        let _ = self
            .hw_context
            .as_mut()
            .context("Hardware decode context is missing")?;
        unsafe {
            let result = ffmpeg::ffi::av_hwframe_transfer_data(
                self.sw_frame.as_mut_ptr(),
                self.decoded_frame.as_ptr(),
                0,
            );
            if result < 0 {
                bail!("Failed to transfer hardware frame to CPU: {}", result);
            }
            Ok(())
        }
    }

    fn frame_pts_secs(&self) -> f64 {
        let raw = unsafe { &*self.decoded_frame.as_ptr() };
        let pts = if raw.best_effort_timestamp != ffmpeg::ffi::AV_NOPTS_VALUE {
            raw.best_effort_timestamp
        } else if raw.pts != ffmpeg::ffi::AV_NOPTS_VALUE {
            raw.pts
        } else if raw.pkt_dts != ffmpeg::ffi::AV_NOPTS_VALUE {
            raw.pkt_dts
        } else {
            return self.last_decoded_pts_secs;
        };

        if self.stream_time_base_den == 0 {
            return self.last_decoded_pts_secs;
        }

        let pts_secs =
            pts as f64 * self.stream_time_base_num as f64 / self.stream_time_base_den as f64;
        if pts_secs.is_finite() {
            pts_secs.max(0.0)
        } else {
            self.last_decoded_pts_secs
        }
    }
}

// ===========================================================================
// Audio support
// ===========================================================================

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
        Some(self.samples.len() - self.next_index)
    }
    fn channels(&self) -> u16 {
        self.channels
    }
    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
    fn total_duration(&self) -> Option<Duration> {
        let remaining_samples = (self.samples.len() - self.next_index) as f64;
        let seconds = remaining_samples / (self.channels as f64 * self.sample_rate as f64);
        Some(Duration::from_secs_f64(seconds))
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
    use ffmpeg_next::media::Type;

    let mut input = ffmpeg::format::input(video_path)
        .context("Failed to open video file for audio decoding")?;

    let audio_stream_idx = input
        .streams()
        .enumerate()
        .find(|(_, s)| s.parameters().medium() == Type::Audio)
        .map(|(idx, _)| idx)
        .ok_or_else(|| anyhow!("No audio stream found in video"))?;

    let stream = input
        .stream(audio_stream_idx)
        .ok_or_else(|| anyhow!("Cannot access audio stream"))?;
    let mut decoder = ffmpeg::codec::context::Context::from_parameters(stream.parameters())?
        .decoder()
        .audio()?;

    let in_channel_layout = decoder.channel_layout();
    let in_format = decoder.format();

    let out_sample_format = ffmpeg::format::Sample::I16(ffmpeg_next::format::sample::Type::Packed);
    let mut resampler = ffmpeg::software::resampling::Context::get(
        in_format,
        in_channel_layout,
        decoder.rate(),
        out_sample_format,
        ffmpeg::util::channel_layout::ChannelLayout::STEREO,
        48_000,
    )
    .context("Failed to create audio resampler")?;

    let mut samples = Vec::new();
    let mut decoded_frame = ffmpeg::util::frame::audio::Audio::empty();

    fn append_packed_i16_samples(dst: &mut Vec<i16>, plane: &[u8], sample_count: usize) {
        let max_pairs = plane.len() / 2;
        let n = sample_count.min(max_pairs);
        if n == 0 {
            return;
        }
        let bytes = &plane[..n * 2];
        let (prefix, aligned, suffix) = unsafe { bytes.align_to::<i16>() };
        if prefix.is_empty() && suffix.is_empty() && aligned.len() == n {
            #[cfg(target_endian = "little")]
            {
                dst.extend_from_slice(aligned);
            }
            #[cfg(target_endian = "big")]
            {
                dst.extend(aligned.iter().copied().map(i16::from_le));
            }
            return;
        }
        dst.reserve(n);
        for chunk in bytes.chunks_exact(2) {
            dst.push(i16::from_le_bytes([chunk[0], chunk[1]]));
        }
    }

    for (_, packet) in input.packets() {
        if packet.stream() == audio_stream_idx {
            let _ = decoder.send_packet(&packet);
            while decoder.receive_frame(&mut decoded_frame).is_ok() {
                let mut resampled = ffmpeg::util::frame::audio::Audio::empty();
                if resampler.run(&decoded_frame, &mut resampled).is_ok() {
                    let plane = resampled.data(0);
                    let sample_count = resampled.samples() * resampled.channels() as usize;
                    append_packed_i16_samples(&mut samples, plane, sample_count);
                }
            }
        }
    }

    let _ = decoder.send_eof();
    let mut decoded_frame = ffmpeg::util::frame::audio::Audio::empty();
    while decoder.receive_frame(&mut decoded_frame).is_ok() {
        let mut resampled = ffmpeg::util::frame::audio::Audio::empty();
        if resampler.run(&decoded_frame, &mut resampled).is_ok() {
            let plane = resampled.data(0);
            let sample_count = resampled.samples() * resampled.channels() as usize;
            append_packed_i16_samples(&mut samples, plane, sample_count);
        }
    }

    if samples.is_empty() {
        return Ok(AudioBuffer {
            sample_rate: 48_000,
            channels: 2,
            samples: vec![],
        });
    }

    Ok(AudioBuffer {
        sample_rate: 48_000,
        channels: 2,
        samples,
    })
}

// ===========================================================================
// Zero-copy RGBA frame → ColorImage conversion
// ===========================================================================

/// Write the scaled RGBA frame directly into a pooled `ColorImage`.
///
/// The FFmpeg scaler output (`self.rgba_frame`) contains RGBA bytes in a
/// stride-padded buffer. We copy row-by-row (or in one contiguous block when
/// the stride matches) directly into the pool-backed `ColorImage`'s pixel
/// storage, zero-copy to the CPU → GPU upload path.
fn rgba_frame_to_color_image(
    frame: &ffmpeg::util::frame::video::Video,
    width: u32,
    height: u32,
    pool: &Arc<FramePool>,
) -> Result<PooledColorImage> {
    let stride = frame.stride(0);
    let data = frame.data(0);
    let row_bytes = width as usize * 4;

    let mut pooled = pool.acquire_color_image(width, height);
    let dst = pooled.raw_rgba_mut();

    let total_size = height as usize * row_bytes;

    if stride == row_bytes {
        // Fast path: single contiguous copy.
        dst[..total_size].copy_from_slice(&data[..total_size]);
    } else {
        // Slow path: stride-padded data, copy row by row.
        for y in 0..height as usize {
            let src_offset = y * stride;
            let dst_offset = y * row_bytes;
            dst[dst_offset..dst_offset + row_bytes]
                .copy_from_slice(&data[src_offset..src_offset + row_bytes]);
        }
    }

    Ok(pooled)
}

fn scaled_dimensions(preview_width: u32, metadata: &VideoFileMetadata) -> (u32, u32) {
    let width = preview_width.min(metadata.width.max(1)).max(1);
    let aspect = metadata.height.max(1) as f64 / metadata.width.max(1) as f64;
    let mut height = (f64::from(width) * aspect).round() as u32;
    height = height.max(1);
    if !height.is_multiple_of(2) {
        height += 1;
    }
    (width, height)
}
