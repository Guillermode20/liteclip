use anyhow::{anyhow, bail, Context, Result};
use crossbeam::channel::{bounded, Receiver, Sender, TryRecvError, TrySendError};
use ffmpeg_next as ffmpeg;
use image::RgbaImage;
use rodio::{Sink, Source};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::output::functions::ffmpeg_executable_path;
use crate::output::VideoFileMetadata;

const FRAME_CHANNEL_CAPACITY: usize = 24; // Increased from 12 for better buffering
const PLAYBACK_QUEUE_DEPTH: usize = 20; // Increased from 10 for smoother playback

pub struct PlaybackFrame {
    pub image: RgbaImage,
}

struct TimedFrame {
    pts_secs: f64,
    image: RgbaImage,
}

pub struct PlaybackController {
    metadata: VideoFileMetadata,
    shared: Arc<SharedPlaybackState>,
    decoder: DecodePipeline,
    audio_handle: Option<rodio::OutputStreamHandle>,
    _audio_shutdown_tx: Option<std::sync::mpsc::Sender<()>>,
}

struct SharedPlaybackState {
    current_time_secs: Mutex<f64>,
    playing_since: Mutex<Option<PlaybackClock>>,
    latest_frame: Mutex<Option<PlaybackFrame>>,
    frame_queue: Mutex<VecDeque<TimedFrame>>,
    last_error: Mutex<Option<String>>,
    request_generation: AtomicU64,
    video_request_in_flight: AtomicBool,
    audio_loading: AtomicBool,
    audio_buffer: Mutex<Option<AudioBuffer>>,
    audio_generation: AtomicU64,
    audio_started_generation: AtomicU64,
}

struct PlaybackClock {
    start_time_secs: f64,
    started_at: Option<Instant>,
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum DecoderFrameKind {
    Preview,
    Playback,
}

struct DecoderFrame {
    request_id: u64,
    kind: DecoderFrameKind,
    pts_secs: f64,
    image: RgbaImage,
}

struct DecoderError {
    request_id: u64,
    message: String,
}

enum DecoderCommand {
    Preview { request_id: u64, time_secs: f64 },
    Playback { request_id: u64, time_secs: f64 },
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
    input: ffmpeg::format::context::Input,
    decoder: ffmpeg::decoder::Video,
    scaler: ffmpeg::software::scaling::Context,
    decoded_frame: ffmpeg::util::frame::video::Video,
    rgba_frame: ffmpeg::util::frame::video::Video,
    stream_index: usize,
    output_width: u32,
    output_height: u32,
    stream_time_base_num: i32,
    stream_time_base_den: i32,
    seek_target_secs: f64,
    last_pts_secs: f64,
    /// Track keyframe positions for smarter seeking
    keyframe_positions: Vec<f64>,
}

impl PlaybackController {
    pub fn new(video_path: PathBuf, metadata: VideoFileMetadata, preview_width: u32) -> Self {
        let (output_width, output_height) = scaled_dimensions(preview_width, &metadata);
        let shared = Arc::new(SharedPlaybackState {
            current_time_secs: Mutex::new(0.0),
            playing_since: Mutex::new(None),
            latest_frame: Mutex::new(None),
            frame_queue: Mutex::new(VecDeque::new()),
            last_error: Mutex::new(None),
            request_generation: AtomicU64::new(1),
            video_request_in_flight: AtomicBool::new(false),
            audio_loading: AtomicBool::new(false),
            audio_buffer: Mutex::new(None),
            audio_generation: AtomicU64::new(1),
            audio_started_generation: AtomicU64::new(0),
        });

        let decoder = DecodePipeline::new(
            video_path.clone(),
            output_width,
            output_height,
            metadata.fps.clamp(1.0, 120.0),
        );

        let (audio_shutdown_tx, audio_shutdown_rx) = std::sync::mpsc::channel();
        let (audio_handle_tx, audio_handle_rx) = std::sync::mpsc::channel();

        thread::spawn(move || {
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
        self.request_preview_at(time_secs);
    }

    pub fn request_preview_frame_fast(&mut self, time_secs: f64) {
        self.request_preview_at(time_secs);
    }

    fn request_preview_at(&mut self, time_secs: f64) {
        let clamped_time = self.clamp_time(time_secs);
        self.stop_audio();
        *self.shared.current_time_secs.lock().unwrap() = clamped_time;
        *self.shared.playing_since.lock().unwrap() = None;
        self.shared.frame_queue.lock().unwrap().clear();
        self.decoder.stop();

        let request_id = self.next_request_id();
        self.shared
            .video_request_in_flight
            .store(true, Ordering::SeqCst);
        if let Err(err) = self.decoder.request_preview(request_id, clamped_time) {
            self.shared
                .video_request_in_flight
                .store(false, Ordering::SeqCst);
            *self.shared.last_error.lock().unwrap() = Some(err.to_string());
        }
    }

    pub fn play_from(&mut self, time_secs: f64) {
        let clamped_time = self.clamp_time(time_secs);
        self.shared.frame_queue.lock().unwrap().clear();
        *self.shared.current_time_secs.lock().unwrap() = clamped_time;
        *self.shared.playing_since.lock().unwrap() = Some(PlaybackClock {
            start_time_secs: clamped_time,
            started_at: None, // Wait for first frame to start clock and audio
        });
        tracing::info!("play_from: starting at {:.3}s", clamped_time);

        let request_id = self.next_request_id();
        self.shared
            .video_request_in_flight
            .store(false, Ordering::SeqCst);
        if let Err(err) = self.decoder.start_playback(request_id, clamped_time) {
            tracing::error!("play_from: failed to start playback: {}", err);
            *self.shared.playing_since.lock().unwrap() = None;
            *self.shared.last_error.lock().unwrap() = Some(err.to_string());
            return;
        }
        self.stop_audio();
    }

    pub fn pause_at(&mut self, time_secs: f64) {
        let clamped_time = self.clamp_time(time_secs);
        *self.shared.current_time_secs.lock().unwrap() = clamped_time;
        *self.shared.playing_since.lock().unwrap() = None;
        self.shared.frame_queue.lock().unwrap().clear();
        self.shared
            .video_request_in_flight
            .store(false, Ordering::SeqCst);
        self.next_request_id();
        self.decoder.stop();
        self.stop_audio();
    }

    pub fn playback_position_secs(&self) -> f64 {
        let maybe_clock = self.shared.playing_since.lock().unwrap();
        if let Some(clock) = maybe_clock.as_ref() {
            if let Some(started_at) = clock.started_at {
                return self.clamp_time(clock.start_time_secs + started_at.elapsed().as_secs_f64());
            }
            return self.clamp_time(clock.start_time_secs);
        }
        *self.shared.current_time_secs.lock().unwrap()
    }

    pub fn is_playing(&self) -> bool {
        self.shared.playing_since.lock().unwrap().is_some()
    }

    pub fn has_pending_activity(&self) -> bool {
        self.is_playing()
            || self.shared.audio_loading.load(Ordering::SeqCst)
            || self.shared.video_request_in_flight.load(Ordering::SeqCst)
    }

    pub fn is_frame_request_in_flight(&self) -> bool {
        self.shared.video_request_in_flight.load(Ordering::SeqCst)
    }

    pub fn take_frame(&self) -> Option<PlaybackFrame> {
        self.shared.latest_frame.lock().unwrap().take()
    }

    pub fn take_playback_frame(&self) -> Option<RgbaImage> {
        let wall_time_secs = self.playback_position_secs();
        let mut queue = self.shared.frame_queue.lock().unwrap();
        if queue.is_empty() {
            return None;
        }

        let frame_duration = 1.0 / self.metadata.fps.max(1.0);

        // Drop frames that are too old (more than 1 frame behind wall time)
        while queue.len() > 1 {
            if let Some(front) = queue.front() {
                if wall_time_secs - front.pts_secs > frame_duration {
                    queue.pop_front();
                } else {
                    break;
                }
            } else {
                break;
            }
        }

        if queue.is_empty() {
            tracing::info!(
                "take_playback_frame: queue empty after dropping old frames, wall={:.3}s",
                wall_time_secs
            );
            return None;
        }

        // Find all frames with pts <= wall_time and take the last one (closest to wall time)
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
            let frame = queue.pop_front().unwrap();
            tracing::info!(
                "take_playback_frame: wall={:.3}s, pts={:.3}s, {} remaining",
                wall_time_secs,
                frame.pts_secs,
                queue.len()
            );
            return Some(frame.image);
        }

        // No frame with pts <= wall_time. Check if the first frame is close enough.
        // Allow up to one frame duration of "early" display to maintain smooth playback.
        if let Some(front) = queue.front() {
            let ahead_by = front.pts_secs - wall_time_secs;
            if ahead_by <= frame_duration {
                let frame = queue.pop_front().unwrap();
                tracing::info!(
                    "take_playback_frame: wall={:.3}s, taking early frame pts={:.3}s (ahead by {:.3}s)",
                    wall_time_secs,
                    frame.pts_secs,
                    ahead_by
                );
                return Some(frame.image);
            }
            tracing::info!(
                "take_playback_frame: wall={:.3}s, waiting for pts={:.3}s (ahead by {:.3}s)",
                wall_time_secs,
                front.pts_secs,
                ahead_by
            );
        }
        None
    }

    pub fn take_error(&self) -> Option<String> {
        self.shared.last_error.lock().unwrap().take()
    }

    pub fn playback_fps(&self) -> f64 {
        self.metadata.fps.clamp(1.0, 120.0)
    }

    pub fn poll(&mut self) {
        let mut frame_count = 0;
        loop {
            {
                let queue = self.shared.frame_queue.lock().unwrap();
                if queue.len() >= PLAYBACK_QUEUE_DEPTH {
                    break;
                }
            }

            let Some(frame) = self.decoder.try_recv_frame() else {
                break;
            };

            if frame.request_id != self.active_request_id() {
                tracing::debug!("poll: skipping frame from old request {}", frame.request_id);
                continue;
            }

            match frame.kind {
                DecoderFrameKind::Preview => {
                    tracing::debug!("poll: received preview frame pts={:.3}s", frame.pts_secs);
                    *self.shared.latest_frame.lock().unwrap() =
                        Some(PlaybackFrame { image: frame.image });
                    self.shared
                        .video_request_in_flight
                        .store(false, Ordering::SeqCst);
                }
                DecoderFrameKind::Playback => {
                    let mut queue = self.shared.frame_queue.lock().unwrap();
                    let pts = frame.pts_secs;
                    queue.push_back(TimedFrame {
                        pts_secs: pts,
                        image: frame.image,
                    });
                    frame_count += 1;
                    if frame_count <= 5 {
                        tracing::info!(
                            "poll: queued frame pts={:.3}s, queue_len={}",
                            pts,
                            queue.len()
                        );
                    }
                }
            }
        }
        if frame_count > 0 {
            tracing::trace!("poll: received {} frames total", frame_count);
        }

        while let Some(error) = self.decoder.try_recv_error() {
            if error.request_id != 0 && error.request_id != self.active_request_id() {
                continue;
            }
            tracing::error!("poll: decoder error: {}", error.message);
            self.shared
                .video_request_in_flight
                .store(false, Ordering::SeqCst);
            *self.shared.last_error.lock().unwrap() = Some(error.message);
        }

        if !self.is_playing() {
            return;
        }

        self.check_and_start_clock();

        let current_time = self.playback_position_secs();
        *self.shared.current_time_secs.lock().unwrap() = current_time;
        if current_time >= self.metadata.duration_secs {
            self.pause_at(self.metadata.duration_secs);
        }
    }

    fn check_and_start_clock(&mut self) {
        let mut clock_unlocked = false;
        {
            let mut maybe_clock = self.shared.playing_since.lock().unwrap();
            if let Some(clock) = maybe_clock.as_mut() {
                if clock.started_at.is_none() {
                    let queue = self.shared.frame_queue.lock().unwrap();
                    if let Some(front) = queue.front() {
                        clock.start_time_secs = front.pts_secs;
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
        let queue = self.shared.frame_queue.lock().unwrap();
        let count = queue.len();
        let mb =
            queue.iter().map(|f| f.image.as_raw().len()).sum::<usize>() as f64 / (1024.0 * 1024.0);
        (count, mb)
    }

    fn clamp_time(&self, time_secs: f64) -> f64 {
        time_secs.clamp(0.0, self.metadata.duration_secs)
    }

    fn begin_audio_preload(&self, video_path: PathBuf) {
        if !self.metadata.has_audio || self.shared.audio_loading.swap(true, Ordering::SeqCst) {
            return;
        }

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

        let Some(handle) = self.audio_handle.clone() else {
            return;
        };

        let start_time = self.playback_position_secs();
        let generation = self.shared.audio_generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.shared
            .audio_started_generation
            .store(generation, Ordering::SeqCst);
        let shared = self.shared.clone();
        thread::spawn(move || {
            let sink = match Sink::try_new(&handle) {
                Ok(sink) => sink,
                Err(err) => {
                    *shared.last_error.lock().unwrap() =
                        Some(format!("Audio output failed: {err}"));
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
        });
    }

    fn stop_audio(&mut self) {
        self.shared.audio_generation.fetch_add(1, Ordering::SeqCst);
    }
}

impl Drop for PlaybackController {
    fn drop(&mut self) {
        let _ = self.shared.playing_since.lock().map(|mut g| *g = None);
        self.shared.audio_generation.fetch_add(1, Ordering::SeqCst);
    }
}

impl DecodePipeline {
    fn new(video_path: PathBuf, output_width: u32, output_height: u32, fps: f64) -> Self {
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
            );
        });
        Self {
            command_tx,
            frame_rx,
            error_rx,
            worker: Some(worker),
        }
    }

    fn request_preview(&self, request_id: u64, time_secs: f64) -> Result<()> {
        self.command_tx
            .send(DecoderCommand::Preview {
                request_id,
                time_secs,
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
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn send_playback_frame(
    command_rx: &Receiver<DecoderCommand>,
    frame_tx: &Sender<DecoderFrame>,
    frame: DecoderFrame,
) -> Result<Option<DecoderCommand>> {
    let mut pending_frame = frame;
    loop {
        match frame_tx.try_send(pending_frame) {
            Ok(()) => {
                return Ok(None);
            }
            Err(TrySendError::Disconnected(_)) => {
                bail!("Playback frame channel disconnected");
            }
            Err(TrySendError::Full(returned_frame)) => {
                pending_frame = returned_frame;
                match command_rx.try_recv() {
                    Ok(command @ DecoderCommand::Shutdown)
                    | Ok(command @ DecoderCommand::Stop)
                    | Ok(command @ DecoderCommand::Preview { .. })
                    | Ok(command @ DecoderCommand::Playback { .. }) => {
                        tracing::debug!(
                            "send_playback_frame: received command during spin: {:?}",
                            command_kind(&command)
                        );
                        return Ok(Some(command));
                    }
                    Err(TryRecvError::Disconnected) => {
                        bail!("Decoder command channel disconnected");
                    }
                    Err(TryRecvError::Empty) => {
                        thread::sleep(Duration::from_millis(1));
                    }
                }
            }
        }
    }
}

fn command_kind(cmd: &DecoderCommand) -> &'static str {
    match cmd {
        DecoderCommand::Shutdown => "Shutdown",
        DecoderCommand::Stop => "Stop",
        DecoderCommand::Preview { .. } => "Preview",
        DecoderCommand::Playback { .. } => "Playback",
    }
}

fn decoder_worker_loop(
    video_path: PathBuf,
    output_width: u32,
    output_height: u32,
    fps: f64,
    command_rx: Receiver<DecoderCommand>,
    frame_tx: Sender<DecoderFrame>,
    error_tx: Sender<DecoderError>,
) {
    tracing::info!("Decoder worker starting for {:?}", video_path);
    let _ = ffmpeg::init();
    let mut session = match DecoderSession::open(&video_path, output_width, output_height, fps) {
        Ok(session) => {
            tracing::info!("Decoder session opened successfully");
            session
        }
        Err(err) => {
            tracing::error!("Failed to initialize video decoder: {err:#}");
            let _ = error_tx.send(DecoderError {
                request_id: 0,
                message: format!("Failed to initialize video decoder: {err:#}"),
            });
            return;
        }
    };

    tracing::info!("Decoder worker entering main loop");
    loop {
        let command = match command_rx.recv() {
            Ok(command) => command,
            Err(_) => {
                tracing::info!("Decoder worker channel disconnected, exiting");
                return;
            }
        };

        match command {
            DecoderCommand::Shutdown => {
                tracing::info!("Decoder worker received shutdown, exiting");
                return;
            }
            DecoderCommand::Stop => {
                tracing::debug!("Decoder worker received stop");
                continue;
            }
            DecoderCommand::Preview {
                request_id,
                time_secs,
            } => {
                tracing::debug!("Preview request {} at {:.2}s", request_id, time_secs);
                if let Err(err) = session.seek_to(time_secs) {
                    tracing::error!("Preview seek failed: {err:#}");
                    let _ = error_tx.send(DecoderError {
                        request_id,
                        message: format!("Preview seek failed: {err:#}"),
                    });
                    continue;
                }

                match session.decode_next_image() {
                    Ok(Some((pts_secs, image))) => {
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
            DecoderCommand::Playback {
                request_id,
                time_secs,
            } => {
                tracing::info!("Playback request {} at {:.2}s", request_id, time_secs);
                if let Err(err) = session.seek_to(time_secs) {
                    tracing::error!("Playback seek failed: {err:#}");
                    let _ = error_tx.send(DecoderError {
                        request_id,
                        message: format!("Playback seek failed: {err:#}"),
                    });
                    continue;
                }

                let mut active_request_id = request_id;
                let mut frame_count = 0u32;
                tracing::info!("Starting playback decode loop");
                'playback: loop {
                    match command_rx.try_recv() {
                        Ok(DecoderCommand::Shutdown) => {
                            tracing::info!(
                                "Playback {} received shutdown after {} frames",
                                active_request_id,
                                frame_count
                            );
                            return;
                        }
                        Ok(DecoderCommand::Stop) => {
                            tracing::debug!(
                                "Playback {} stopped after {} frames",
                                active_request_id,
                                frame_count
                            );
                            break 'playback;
                        }
                        Ok(DecoderCommand::Preview {
                            request_id,
                            time_secs,
                        }) => {
                            if let Err(err) = session.seek_to(time_secs) {
                                let _ = error_tx.send(DecoderError {
                                    request_id,
                                    message: format!("Preview seek failed: {err:#}"),
                                });
                            } else {
                                match session.decode_next_image() {
                                    Ok(Some((pts_secs, image))) => {
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
                                            message: "No preview frame could be decoded"
                                                .to_string(),
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
                            break 'playback;
                        }
                        Ok(DecoderCommand::Playback {
                            request_id,
                            time_secs,
                        }) => {
                            active_request_id = request_id;
                            if let Err(err) = session.seek_to(time_secs) {
                                let _ = error_tx.send(DecoderError {
                                    request_id,
                                    message: format!("Playback seek failed: {err:#}"),
                                });
                                break 'playback;
                            }
                        }
                        Err(TryRecvError::Disconnected) => {
                            tracing::info!(
                                "Playback {} channel disconnected after {} frames",
                                active_request_id,
                                frame_count
                            );
                            return;
                        }
                        Err(TryRecvError::Empty) => {}
                    }

                    match session.decode_next_image() {
                        Ok(Some((pts_secs, image))) => {
                            frame_count += 1;
                            if frame_count <= 3 {
                                tracing::info!(
                                    "Decoded frame #{} pts={:.3}s",
                                    frame_count,
                                    pts_secs
                                );
                            }
                            let frame = DecoderFrame {
                                request_id: active_request_id,
                                kind: DecoderFrameKind::Playback,
                                pts_secs,
                                image,
                            };
                            match send_playback_frame(&command_rx, &frame_tx, frame) {
                                Ok(None) => {}
                                Ok(Some(DecoderCommand::Shutdown)) => {
                                    tracing::info!(
                                        "Playback {} shutdown during send after {} frames",
                                        active_request_id,
                                        frame_count
                                    );
                                    return;
                                }
                                Ok(Some(DecoderCommand::Stop)) => {
                                    break 'playback;
                                }
                                Ok(Some(DecoderCommand::Preview {
                                    request_id,
                                    time_secs,
                                })) => {
                                    if let Err(err) = session.seek_to(time_secs) {
                                        let _ = error_tx.send(DecoderError {
                                            request_id,
                                            message: format!("Preview seek failed: {err:#}"),
                                        });
                                    } else {
                                        match session.decode_next_image() {
                                            Ok(Some((pts_secs, image))) => {
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
                                                    message: "No preview frame could be decoded"
                                                        .to_string(),
                                                });
                                            }
                                            Err(err) => {
                                                let _ = error_tx.send(DecoderError {
                                                    request_id,
                                                    message: format!(
                                                        "Preview decode failed: {err:#}"
                                                    ),
                                                });
                                            }
                                        }
                                    }
                                    break 'playback;
                                }
                                Ok(Some(DecoderCommand::Playback {
                                    request_id,
                                    time_secs,
                                })) => {
                                    active_request_id = request_id;
                                    if let Err(err) = session.seek_to(time_secs) {
                                        let _ = error_tx.send(DecoderError {
                                            request_id,
                                            message: format!("Playback seek failed: {err:#}"),
                                        });
                                        break 'playback;
                                    }
                                }
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

impl DecoderSession {
    fn open(video_path: &Path, output_width: u32, output_height: u32, _fps: f64) -> Result<Self> {
        let input = ffmpeg::format::input(video_path)
            .with_context(|| format!("Failed to open video file: {video_path:?}"))?;
        let input_stream = input
            .streams()
            .best(ffmpeg::media::Type::Video)
            .context("No video stream found")?;
        let stream_index = input_stream.index();
        let stream_time_base = input_stream.time_base();
        let context = ffmpeg::codec::context::Context::from_parameters(input_stream.parameters())
            .context("Failed to create decoder context")?;
        let decoder = context
            .decoder()
            .video()
            .context("Failed to open video decoder")?;

        let input_format = decoder.format();
        let input_width = decoder.width();
        let input_height = decoder.height();
        let scaler = ffmpeg::software::scaling::Context::get(
            input_format,
            input_width,
            input_height,
            ffmpeg::format::Pixel::RGBA,
            output_width,
            output_height,
            ffmpeg::software::scaling::flag::Flags::BILINEAR,
        )
        .context("Failed to create decoder scaler")?;

        Ok(Self {
            input,
            decoder,
            scaler,
            decoded_frame: ffmpeg::util::frame::video::Video::empty(),
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
            last_pts_secs: 0.0,
            keyframe_positions: Vec::new(),
        })
    }

    fn seek_to(&mut self, time_secs: f64) -> Result<()> {
        // Find the nearest keyframe before the target time for more efficient seeking
        let seek_time = if !self.keyframe_positions.is_empty() {
            // Find the keyframe just before our target time
            let nearest_keyframe = self
                .keyframe_positions
                .iter()
                .filter(|&&k| k <= time_secs)
                .last()
                .copied();

            match nearest_keyframe {
                Some(kf) if (time_secs - kf) > 2.0 => {
                    // If we're more than 2 seconds past a keyframe, seek to keyframe first
                    tracing::debug!(
                        "Seeking to keyframe at {:.3}s instead of {:.3}s",
                        kf,
                        time_secs
                    );
                    kf
                }
                _ => time_secs,
            }
        } else {
            time_secs
        };

        let timestamp = (seek_time.max(0.0) * ffmpeg::ffi::AV_TIME_BASE as f64).round() as i64;
        let result = unsafe {
            ffmpeg::ffi::av_seek_frame(
                self.input.as_mut_ptr(),
                -1,
                timestamp,
                ffmpeg::ffi::AVSEEK_FLAG_BACKWARD,
            )
        };
        if result < 0 {
            bail!("seek failed with error code {result}");
        }
        unsafe {
            ffmpeg::ffi::avformat_flush(self.input.as_mut_ptr());
            ffmpeg::ffi::avcodec_flush_buffers(self.decoder.as_mut_ptr());
        }
        self.seek_target_secs = time_secs.max(0.0);
        self.last_pts_secs = self.seek_target_secs;
        Ok(())
    }

    fn decode_next_image(&mut self) -> Result<Option<(f64, RgbaImage)>> {
        loop {
            match self.decoder.receive_frame(&mut self.decoded_frame) {
                Ok(()) => {
                    let pts_secs = self.frame_pts_secs();

                    // Check if this is a keyframe
                    let is_keyframe = unsafe {
                        let flags = (*self.decoded_frame.as_ptr()).flags;
                        flags & ffmpeg::ffi::AV_FRAME_FLAG_KEY != 0
                    };

                    // Track keyframe positions for smarter seeking
                    if is_keyframe {
                        self.keyframe_positions.push(pts_secs);
                        // Keep keyframe list sorted and remove duplicates
                        self.keyframe_positions
                            .sort_by(|a, b| a.partial_cmp(b).unwrap());
                        self.keyframe_positions
                            .dedup_by(|a, b| (*a - *b).abs() < 0.001);
                        // Limit the list size to prevent memory bloat
                        if self.keyframe_positions.len() > 1000 {
                            self.keyframe_positions.remove(0);
                        }
                    }

                    tracing::trace!(
                        "decode_next_image: received frame pts={:.3}s, seek_target={:.3}s, keyframe={}",
                        pts_secs,
                        self.seek_target_secs,
                        is_keyframe
                    );

                    if pts_secs + 0.001 < self.seek_target_secs {
                        tracing::trace!("decode_next_image: skipping frame before seek target");
                        continue;
                    }

                    self.scaler
                        .run(&self.decoded_frame, &mut self.rgba_frame)
                        .context("Failed to scale decoded frame")?;
                    let image = rgba_frame_to_image(
                        &self.rgba_frame,
                        self.output_width,
                        self.output_height,
                    )?;
                    self.last_pts_secs = pts_secs;
                    return Ok(Some((pts_secs, image)));
                }
                Err(ffmpeg::Error::Other { errno }) if errno == ffmpeg::util::error::EAGAIN => {}
                Err(ffmpeg::Error::Eof) => {
                    tracing::debug!("decode_next_image: EOF reached");
                    return Ok(None);
                }
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

    fn frame_pts_secs(&self) -> f64 {
        let raw = unsafe { &*self.decoded_frame.as_ptr() };
        let pts = if raw.best_effort_timestamp != ffmpeg::ffi::AV_NOPTS_VALUE {
            raw.best_effort_timestamp
        } else if raw.pts != ffmpeg::ffi::AV_NOPTS_VALUE {
            raw.pts
        } else if raw.pkt_dts != ffmpeg::ffi::AV_NOPTS_VALUE {
            raw.pkt_dts
        } else {
            return self.last_pts_secs;
        };

        if self.stream_time_base_den == 0 {
            return self.last_pts_secs;
        }

        let pts_secs =
            pts as f64 * self.stream_time_base_num as f64 / self.stream_time_base_den as f64;
        if pts_secs.is_finite() {
            pts_secs.max(0.0)
        } else {
            self.last_pts_secs
        }
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
    let ffmpeg = ffmpeg_executable_path();
    let output = Command::new(&ffmpeg)
        .args([
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
        ])
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

fn rgba_frame_to_image(
    frame: &ffmpeg::util::frame::video::Video,
    width: u32,
    height: u32,
) -> Result<RgbaImage> {
    let stride = frame.stride(0);
    let data = frame.data(0);
    let row_bytes = width as usize * 4;
    let mut rgba = vec![0_u8; row_bytes * height as usize];

    for y in 0..height as usize {
        let src_offset = y * stride;
        let dst_offset = y * row_bytes;
        rgba[dst_offset..dst_offset + row_bytes]
            .copy_from_slice(&data[src_offset..src_offset + row_bytes]);
    }

    RgbaImage::from_raw(width, height, rgba).context("Failed to create RGBA image from frame")
}

fn scaled_dimensions(preview_width: u32, metadata: &VideoFileMetadata) -> (u32, u32) {
    let width = preview_width.min(metadata.width.max(1)).max(1);
    let aspect = metadata.height.max(1) as f64 / metadata.width.max(1) as f64;
    let mut height = (f64::from(width) * aspect).round() as u32;
    height = height.max(1);
    if height % 2 != 0 {
        height += 1;
    }
    (width, height)
}
