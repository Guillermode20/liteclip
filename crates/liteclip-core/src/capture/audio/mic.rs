//! WASAPI Microphone Capture
//!
//! Captures microphone audio using WASAPI in shared mode.

use anyhow::{Context, Result};
use bytes::BytesMut;
use crossbeam::channel::{bounded, Receiver, Sender};
use nnnoiseless::DenoiseState;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tracing::{debug, error, warn};

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Media::Audio::{
    eCapture, eConsole, IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator, MMDeviceEnumerator,
    AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY, AUDCLNT_BUFFERFLAGS_SILENT,
    AUDCLNT_BUFFERFLAGS_TIMESTAMP_ERROR, AUDCLNT_SHAREMODE_SHARED,
    AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM, AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
    AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, WAVEFORMATEX,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED,
};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

use crate::buffer::ring::qpc_frequency;
use crate::encode::{EncodedPacket, StreamType};

/// Configuration for WASAPI microphone capture
#[derive(Debug, Clone)]
pub struct WasapiMicConfig {
    pub sample_rate: u32,
    pub channels: u16,
    pub bits_per_sample: u16,
    pub buffer_duration: Duration,
    pub device_id: Option<String>, // None for default device
    pub noise_reduction_enabled: bool,
}

impl Default for WasapiMicConfig {
    fn default() -> Self {
        Self {
            sample_rate: 48000,
            channels: 2,
            bits_per_sample: 16,
            buffer_duration: Duration::from_millis(20),
            device_id: None,
            noise_reduction_enabled: true,
        }
    }
}

/// WASAPI microphone capture implementation
pub struct WasapiMicCapture {
    running: Arc<AtomicBool>,
    initialized: Arc<AtomicBool>,
    packet_tx: Sender<EncodedPacket>,
    packet_rx: Receiver<EncodedPacket>,
    processed_samples: Arc<AtomicU64>,
    capture_thread: Option<thread::JoinHandle<()>>,
}

impl WasapiMicCapture {
    /// Create a new WASAPI microphone capture instance
    pub fn new() -> Result<Self> {
        let (packet_tx, packet_rx) = bounded(128);
        Ok(Self {
            running: Arc::new(AtomicBool::new(false)),
            initialized: Arc::new(AtomicBool::new(false)),
            packet_tx,
            packet_rx,
            processed_samples: Arc::new(AtomicU64::new(0)),
            capture_thread: None,
        })
    }

    /// Start microphone capture
    pub fn start(&mut self, config: WasapiMicConfig) -> Result<()> {
        if self.running.load(Ordering::SeqCst) {
            return Ok(());
        }

        self.running.store(true, Ordering::SeqCst);
        let running = self.running.clone();
        let initialized = self.initialized.clone();
        let packet_tx = self.packet_tx.clone();
        let processed_samples = self.processed_samples.clone();

        let handle = thread::spawn(move || {
            if let Err(e) = Self::capture_loop(
                config,
                running.clone(),
                initialized,
                packet_tx,
                processed_samples,
            ) {
                error!("Microphone capture loop error: {:?}", e);
                running.store(false, Ordering::SeqCst);
            }
        });

        self.capture_thread = Some(handle);

        // Wait for initialization to complete or fail
        let mut attempts = 0;
        while !self.initialized.load(Ordering::SeqCst) && self.running.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_millis(50));
            attempts += 1;
            if attempts > 40 {
                // 2 seconds
                return Err(anyhow::anyhow!(
                    "Microphone capture initialization timed out"
                ));
            }
        }

        if !self.running.load(Ordering::SeqCst) {
            return Err(anyhow::anyhow!("Microphone capture failed to start"));
        }

        Ok(())
    }

    /// Stop microphone capture
    pub fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(handle) = self.capture_thread.take() {
            let _ = handle.join();
        }
        self.initialized.store(false, Ordering::SeqCst);
    }

    /// Get the receiver for captured audio packets
    pub fn receiver(&self) -> Receiver<EncodedPacket> {
        self.packet_rx.clone()
    }

    /// Get total samples processed
    pub fn processed_samples(&self) -> u64 {
        self.processed_samples.load(Ordering::SeqCst)
    }

    fn capture_loop(
        config: WasapiMicConfig,
        running: Arc<AtomicBool>,
        initialized: Arc<AtomicBool>,
        packet_tx: Sender<EncodedPacket>,
        processed_samples: Arc<AtomicU64>,
    ) -> Result<()> {
        let _com = ComApartment::new(COINIT_MULTITHREADED)?;
        Self::set_audio_thread_priority();

        let enumerator: IMMDeviceEnumerator =
            unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }?;

        let device = match &config.device_id {
            Some(id) => {
                let mut id_wide: Vec<u16> = id.encode_utf16().collect();
                id_wide.push(0);
                unsafe { enumerator.GetDevice(windows::core::PCWSTR(id_wide.as_ptr())) }
                    .context("Failed to get microphone device by ID")?
            }
            None => unsafe { enumerator.GetDefaultAudioEndpoint(eCapture, eConsole) }
                .context("Failed to get default microphone device")?,
        };

        // Log which device was selected
        crate::capture::audio::device_info::log_device("Selected microphone device", &device);

        let audio_client: IAudioClient = unsafe { device.Activate(CLSCTX_ALL, None) }
            .context("Failed to activate IAudioClient for microphone")?;

        let block_align = config
            .channels
            .saturating_mul(config.bits_per_sample / 8)
            .max(2);
        let format = WAVEFORMATEX {
            wFormatTag: 1, // PCM
            nChannels: config.channels,
            nSamplesPerSec: config.sample_rate,
            nAvgBytesPerSec: config.sample_rate.saturating_mul(block_align as u32),
            nBlockAlign: block_align,
            wBitsPerSample: config.bits_per_sample,
            cbSize: 0,
        };

        let buffer_hns = duration_to_hns(config.buffer_duration);
        let stream_flags = AUDCLNT_STREAMFLAGS_EVENTCALLBACK
            | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
            | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;

        unsafe {
            audio_client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                stream_flags,
                buffer_hns,
                0,
                &format,
                None,
            )
        }
        .context("Failed to initialize IAudioClient for microphone capture")?;

        let capture_event =
            EventHandle::new().context("Failed to create microphone audio event handle")?;
        unsafe { audio_client.SetEventHandle(capture_event.raw()) }
            .context("Failed to bind microphone audio event handle")?;

        let capture_client: IAudioCaptureClient = unsafe { audio_client.GetService() }
            .context("Failed to get IAudioCaptureClient service for microphone")?;

        unsafe { audio_client.Start() }.context("Failed to start microphone capture")?;
        let wait_timeout_ms = config.buffer_duration.as_millis().clamp(1, 25) as u32;

        // Signal that WASAPI initialization succeeded
        initialized.store(true, Ordering::SeqCst);
        debug!("WASAPI microphone capture loop initialized successfully");

        let start_qpc = query_qpc()?;
        let qpc_freq = qpc_frequency() as f64;
        let sample_rate = config.sample_rate.max(1) as f64;
        tracing::info!(
            "RNNoise: Config - enabled: {}, bits: {}, channels: {}",
            config.noise_reduction_enabled,
            config.bits_per_sample,
            config.channels
        );

        let noise_tx: Option<crossbeam::channel::Sender<RawMicFrame>> =
            if config.bits_per_sample == 16 && config.noise_reduction_enabled {
                tracing::warn!(
                    "RNNoise: Starting noise thread for {} channels",
                    config.channels
                );
                let (tx, rx) = crossbeam::channel::bounded::<RawMicFrame>(64);
                let processor = RNNoiseProcessor::new(config.channels as usize);
                let packet_tx_noise = packet_tx.clone();
                let running_noise = running.clone();
                let processed_samples_noise = processed_samples.clone();
                thread::spawn(move || {
                    run_noise_thread(
                        rx,
                        packet_tx_noise,
                        processor,
                        running_noise,
                        processed_samples_noise,
                    );
                });
                Some(tx)
            } else {
                tracing::warn!(
                    "RNNoise: Processor NOT initialized (bits: {}, enabled: {})",
                    config.bits_per_sample,
                    config.noise_reduction_enabled
                );
                None
            };
        if noise_tx.is_none() {
            tracing::info!("Microphone raw passthrough enabled (noise reduction disabled)");
        }

        let mut total_frames: u64 = 0;
        let mut capture_discontinuities: u64 = 0;
        let mut timestamp_errors: u64 = 0;

        let max_buffer_size = (config.sample_rate as usize / 10) * block_align as usize;
        let mut audio_buffer = BytesMut::with_capacity(max_buffer_size);
        let mut packet_counter: u64 = 0;

        while running.load(Ordering::SeqCst) {
            let mut packet_frames = unsafe { capture_client.GetNextPacketSize() }?;

            if packet_frames == 0 {
                match unsafe { WaitForSingleObject(capture_event.raw(), wait_timeout_ms) }.0 {
                    0 => {}
                    258 => continue,
                    status => {
                        warn!(
                            "Microphone audio wait returned unexpected status: {:?}",
                            status
                        );
                        continue;
                    }
                }
                continue;
            }

            while packet_frames > 0 {
                let mut data_ptr = std::ptr::null_mut();
                let mut frame_count = 0u32;
                let mut flags = 0u32;
                let mut device_position = 0u64;
                let mut qpc_position = 0u64;

                unsafe {
                    capture_client.GetBuffer(
                        &mut data_ptr,
                        &mut frame_count,
                        &mut flags,
                        Some(&mut device_position),
                        Some(&mut qpc_position),
                    )
                }
                .context("IAudioCaptureClient::GetBuffer failed")?;

                let byte_count = frame_count as usize * block_align as usize;
                audio_buffer.resize(byte_count, 0);
                let silent = flags & (AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) != 0;
                unsafe {
                    if !silent && !data_ptr.is_null() {
                        std::ptr::copy_nonoverlapping(
                            data_ptr,
                            audio_buffer.as_mut_ptr(),
                            byte_count,
                        );
                    }
                }

                unsafe { capture_client.ReleaseBuffer(frame_count) }
                    .context("IAudioCaptureClient::ReleaseBuffer failed")?;

                if flags & (AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY.0 as u32) != 0 {
                    capture_discontinuities = capture_discontinuities.saturating_add(1);
                    // Skip warning for first discontinuity (common at startup)
                    if capture_discontinuities > 1 && capture_discontinuities % 100 == 0 {
                        warn!(
                            "Microphone capture discontinuity detected (count={})",
                            capture_discontinuities
                        );
                    }
                }

                let has_timestamp_error =
                    flags & (AUDCLNT_BUFFERFLAGS_TIMESTAMP_ERROR.0 as u32) != 0;
                if has_timestamp_error {
                    timestamp_errors = timestamp_errors.saturating_add(1);
                    if timestamp_errors == 1 || timestamp_errors % 100 == 0 {
                        warn!(
                            "Microphone capture timestamp error detected; using frame-derived timing (count={})",
                            timestamp_errors
                        );
                    }
                }

                let pts = if !has_timestamp_error && qpc_position > 0 {
                    qpc_position.min(i64::MAX as u64) as i64
                } else {
                    start_qpc + ((total_frames as f64 / sample_rate) * qpc_freq) as i64
                };
                total_frames = total_frames.saturating_add(frame_count as u64);

                if let Some(ref tx) = noise_tx {
                    let raw = RawMicFrame {
                        data: audio_buffer.split_to(byte_count),
                        pts,
                        frame_count,
                        silent,
                    };
                    if tx.send(raw).is_err() {
                        running.store(false, Ordering::SeqCst);
                        break;
                    }
                } else {
                    let packet = EncodedPacket::new(
                        audio_buffer.split_to(byte_count).freeze(),
                        pts,
                        pts,
                        false,
                        StreamType::Microphone,
                    );
                    if packet_tx.send(packet).is_err() {
                        running.store(false, Ordering::SeqCst);
                        break;
                    }
                    processed_samples.fetch_add(frame_count as u64, Ordering::Relaxed);
                }

                packet_counter = packet_counter.saturating_add(1);
                if packet_counter.is_multiple_of(MIC_BUFFER_SHRINK_INTERVAL_PACKETS)
                    && audio_buffer.capacity()
                        > max_buffer_size.saturating_mul(MIC_BUFFER_SHRINK_MULTIPLIER)
                {
                    audio_buffer = BytesMut::with_capacity(max_buffer_size);
                }

                packet_frames = unsafe { capture_client.GetNextPacketSize() }?;
            }
        }
        Ok(())
    }
}

fn query_qpc() -> Result<i64> {
    let mut qpc = 0i64;
    unsafe {
        windows::Win32::System::Performance::QueryPerformanceCounter(&mut qpc)
            .context("QueryPerformanceCounter failed")?;
    }
    Ok(qpc)
}

fn duration_to_hns(duration: Duration) -> i64 {
    (duration.as_nanos() / 100) as i64
}

struct EventHandle(HANDLE);

impl EventHandle {
    fn new() -> Result<Self> {
        let handle = unsafe { CreateEventW(None, false, false, None) }?;
        Ok(Self(handle))
    }

    fn raw(&self) -> HANDLE {
        self.0
    }
}

impl Drop for EventHandle {
    fn drop(&mut self) {
        let _ = unsafe { CloseHandle(self.0) };
    }
}

struct ComApartment;

impl ComApartment {
    fn new(mode: windows::Win32::System::Com::COINIT) -> Result<Self> {
        unsafe { CoInitializeEx(None, mode) }.ok()?;
        Ok(Self)
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        unsafe {
            CoUninitialize();
        }
    }
}

impl Drop for WasapiMicCapture {
    fn drop(&mut self) {
        self.stop();
    }
}

impl WasapiMicCapture {
    fn set_audio_thread_priority() {
        #[cfg(windows)]
        {
            use windows::Win32::System::Threading::{
                GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_ABOVE_NORMAL,
            };
            unsafe {
                if let Err(e) = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_ABOVE_NORMAL)
                {
                    warn!("Failed to set microphone audio thread priority: {}", e);
                }
            }
        }
    }
}

const AUDIO_FRAME_SIZE: usize = 480;
const MIC_BUFFER_SHRINK_INTERVAL_PACKETS: u64 = 1024;
const MIC_BUFFER_SHRINK_MULTIPLIER: usize = 2;
const RNNOISE_QUEUE_BASE_CAPACITY: usize = AUDIO_FRAME_SIZE * 16;
const RNNOISE_QUEUE_SHRINK_THRESHOLD: usize = AUDIO_FRAME_SIZE * 64;

struct RawMicFrame {
    data: BytesMut,
    pts: i64,
    frame_count: u32,
    silent: bool,
}

fn run_noise_thread(
    raw_rx: crossbeam::channel::Receiver<RawMicFrame>,
    packet_tx: Sender<EncodedPacket>,
    mut processor: RNNoiseProcessor,
    running: Arc<AtomicBool>,
    processed_samples: Arc<AtomicU64>,
) {
    let mut last_log = std::time::Instant::now();
    let mut packets: u32 = 0;
    loop {
        match raw_rx.recv_timeout(Duration::from_millis(200)) {
            Ok(mut frame) => {
                if !frame.silent {
                    processor.process(&mut frame.data);
                }
                packets += 1;
                if last_log.elapsed() >= Duration::from_secs(5) {
                    tracing::info!(
                        "RNNoise: {} pkts/5s | gate={:.3} presence={:.3} hold={}",
                        packets,
                        processor.gate_gain,
                        processor.speech_presence,
                        processor.hold_counter,
                    );
                    packets = 0;
                    last_log = std::time::Instant::now();
                }
                let frozen = frame.data.freeze();
                let packet =
                    EncodedPacket::new(frozen, frame.pts, frame.pts, false, StreamType::Microphone);
                processed_samples.fetch_add(frame.frame_count as u64, Ordering::Relaxed);
                if packet_tx.send(packet).is_err() {
                    break;
                }
            }
            Err(crossbeam::channel::RecvTimeoutError::Timeout) => {
                if !running.load(Ordering::Relaxed) {
                    break;
                }
            }
            Err(_) => break,
        }
    }
}

struct RNNoiseProcessor {
    channels: usize,
    /// Single RNNoise pass — chaining two passes doubles CPU cost without quality gain.
    state: Box<DenoiseState<'static>>,
    in_buf: Vec<f32>,
    in_head: usize,
    out_buf: Vec<f32>,
    out_head: usize,
    discard_warmup_frame: bool,
    /// Discord-style adaptive noise gate state
    gate_gain: f32,
    /// Smoothed speech probability for adaptive behavior
    speech_presence: f32,
    /// Hold time counter (RNNoise frames, each 10ms)
    hold_counter: u32,
    /// High-pass filter state (single-pole, ~80 Hz at 48 kHz)
    hp_x_prev: f32,
    hp_y_prev: f32,
    frame_in: Box<[f32; AUDIO_FRAME_SIZE]>,
    frame_out: Box<[f32; AUDIO_FRAME_SIZE]>,
}

impl RNNoiseProcessor {
    // ── Discord-style adaptive noise gate parameters ──
    //
    // Hysteresis: higher threshold to open, lower to close (prevents chattering)
    // Adaptive hold: keeps gate open briefly after speech ends
    // Smooth attack/release for natural sound

    /// Smoothed speech presence must exceed this to OPEN the gate
    const GATE_OPEN_THRESHOLD: f32 = 0.55;
    /// Smoothed speech presence must drop below this to START closing
    const GATE_CLOSE_THRESHOLD: f32 = 0.35;
    /// Raw speech_prob above this = high-confidence speech
    const SPEECH_CONFIDENCE_THRESHOLD: f32 = 0.70;

    /// Attack: how fast gate opens  (higher = faster, 1.0 = instant)
    const GATE_ATTACK: f32 = 0.35;
    /// Release: normal close speed when in "maybe" zone
    const GATE_RELEASE: f32 = 0.065;
    /// Faster release when smoothed presence is clearly below close threshold
    const GATE_FAST_RELEASE: f32 = 0.12;

    /// Gain when gate is fully open
    const GATE_MAX_GAIN: f32 = 1.0;
    /// Gain when gate is fully closed (near-silent floor)
    const GATE_FLOOR: f32 = 0.001;

    /// Hold time after speech drops below open threshold (RNNoise frames, 10ms each)
    const HOLD_FRAMES: u32 = 12; // ~120 ms
    /// Extended hold after high-confidence speech (prevents cutting breaths)
    const HOLD_FRAMES_EXTENDED: u32 = 20; // ~200 ms

    /// Smoothing coefficient for speech_presence tracker
    const SPEECH_PRESENCE_SMOOTH: f32 = 0.18;

    /// High-pass filter coefficient for ~80 Hz at 48 kHz.
    const HP_COEFF: f32 = 0.9895;

    fn new(channels: usize) -> Self {
        let in_buf = Vec::with_capacity(AUDIO_FRAME_SIZE * 16);
        let out_buf = Vec::with_capacity(AUDIO_FRAME_SIZE * 16);
        Self {
            channels,
            state: DenoiseState::new(),
            in_buf,
            in_head: 0,
            out_buf,
            out_head: 0,
            discard_warmup_frame: true,
            gate_gain: Self::GATE_FLOOR,
            speech_presence: 0.0,
            hold_counter: 0,
            hp_x_prev: 0.0,
            hp_y_prev: 0.0,
            frame_in: Box::new([0.0; AUDIO_FRAME_SIZE]),
            frame_out: Box::new([0.0; AUDIO_FRAME_SIZE]),
        }
    }

    #[inline]
    fn compact_queue(buf: &mut Vec<f32>, head: &mut usize) {
        if *head > 0 && *head >= buf.len() / 2 {
            let remaining = buf.len() - *head;
            buf.copy_within(*head.., 0);
            buf.truncate(remaining);
            *head = 0;
        }

        if buf.capacity() > RNNOISE_QUEUE_SHRINK_THRESHOLD
            && buf.len().saturating_mul(4) < buf.capacity()
        {
            buf.shrink_to(buf.len().max(RNNOISE_QUEUE_BASE_CAPACITY));
        }
    }

    #[inline]
    fn clamp_i16(sample: f32) -> i16 {
        sample.clamp(-32768.0, 32767.0).round() as i16
    }

    /// Write denoised+gated mono output directly to all channels.
    /// Mic audio is effectively mono — broadcasting preserves quality.
    #[inline]
    fn write_denoised_output(&mut self, samples: &mut [i16], available: usize) {
        let oh = self.out_head;
        let out_slice = &self.out_buf[oh..oh + available];

        // Fast path: if gate is fully closed, just zero out the samples
        if self.gate_gain <= Self::GATE_FLOOR + 0.0001 {
            let total_samples = available * self.channels;
            samples[..total_samples].fill(0);
            return;
        }

        match self.channels {
            1 => {
                for (s, &v) in samples.iter_mut().zip(out_slice) {
                    *s = Self::clamp_i16(v);
                }
            }
            2 => {
                for (chunk, &v) in samples.chunks_exact_mut(2).zip(out_slice) {
                    let s = Self::clamp_i16(v);
                    chunk[0] = s;
                    chunk[1] = s;
                }
            }
            _ => {
                for (chunk, &v) in samples.chunks_exact_mut(self.channels).zip(out_slice) {
                    let s = Self::clamp_i16(v);
                    for ch in chunk {
                        *ch = s;
                    }
                }
            }
        }
    }

    #[allow(clippy::needless_range_loop)]
    fn process(&mut self, data: &mut [u8]) {
        if self.channels == 0 || data.len() % (self.channels * 2) != 0 {
            return;
        }

        let sample_count = data.len() / 2;
        let frame_count = sample_count / self.channels;
        let samples =
            unsafe { std::slice::from_raw_parts_mut(data.as_mut_ptr() as *mut i16, sample_count) };

        // Convert interleaved input to mono and apply HP filter directly into in_buf
        let start_idx = self.in_buf.len();
        self.in_buf.resize(start_idx + frame_count, 0.0);
        let dest = &mut self.in_buf[start_idx..];

        let mut hp_x = self.hp_x_prev;
        let mut hp_y = self.hp_y_prev;

        match self.channels {
            1 => {
                for (d, &s) in dest.iter_mut().zip(samples.iter()) {
                    let mono = s as f32;
                    let y = Self::HP_COEFF * (hp_y + mono - hp_x);
                    hp_x = mono;
                    hp_y = y;
                    *d = y;
                }
            }
            2 => {
                for (d, chunk) in dest.iter_mut().zip(samples.chunks_exact(2)) {
                    let mono = (chunk[0] as f32 + chunk[1] as f32) * 0.5;
                    let y = Self::HP_COEFF * (hp_y + mono - hp_x);
                    hp_x = mono;
                    hp_y = y;
                    *d = y;
                }
            }
            _ => {
                let channels_f = self.channels as f32;
                for (d, chunk) in dest.iter_mut().zip(samples.chunks_exact(self.channels)) {
                    let mut sum = 0.0f32;
                    for &s in chunk {
                        sum += s as f32;
                    }
                    let mono = sum / channels_f;
                    let y = Self::HP_COEFF * (hp_y + mono - hp_x);
                    hp_x = mono;
                    hp_y = y;
                    *d = y;
                }
            }
        }
        self.hp_x_prev = hp_x;
        self.hp_y_prev = hp_y;

        // Process complete RNNoise frames (480 samples = 10 ms each)
        while (self.in_buf.len() - self.in_head) >= AUDIO_FRAME_SIZE {
            let start = self.in_head;
            // nnnoiseless process_frame expects exactly 480 samples.
            // We use frame_in and frame_out to ensure memory alignment and contiguous slices.
            self.frame_in
                .copy_from_slice(&self.in_buf[start..start + AUDIO_FRAME_SIZE]);

            let speech_prob = self
                .state
                .process_frame(self.frame_out.as_mut_slice(), self.frame_in.as_slice());

            self.update_adaptive_gate(speech_prob);

            if self.discard_warmup_frame {
                self.discard_warmup_frame = false;
            } else {
                // Apply adaptive gate gain to denoised output
                if self.gate_gain < 0.999 {
                    let g = self.gate_gain;
                    for s in self.frame_out.iter_mut() {
                        *s *= g;
                    }
                }
                self.out_buf.extend_from_slice(self.frame_out.as_slice());
            }

            self.in_head += AUDIO_FRAME_SIZE;
        }
        Self::compact_queue(&mut self.in_buf, &mut self.in_head);

        // Write denoised mono directly to all output channels.
        let available = (self.out_buf.len() - self.out_head).min(frame_count);
        self.write_denoised_output(samples, available);

        if available > 0 {
            self.out_head += available;
            Self::compact_queue(&mut self.out_buf, &mut self.out_head);
        }

        // For remaining frames with no denoised output yet, we leave original as is.
        // HP filter is already applied to input but only for RNNoise queue.
        // Passthrough is only at the start of the stream (warmup).
    }

    /// Discord-style adaptive noise gate.
    ///
    /// Uses RNNoise speech_prob directly (it already adapts to ambient noise).
    /// Features: hysteresis open/close thresholds, adaptive hold time,
    /// confidence-dependent attack/release rates.
    #[inline]
    fn update_adaptive_gate(&mut self, speech_prob: f32) {
        // Smooth speech probability for stable gating decisions
        self.speech_presence += (speech_prob - self.speech_presence) * Self::SPEECH_PRESENCE_SMOOTH;

        let is_high_confidence = speech_prob > Self::SPEECH_CONFIDENCE_THRESHOLD;
        let is_speech = self.speech_presence > Self::GATE_OPEN_THRESHOLD || is_high_confidence;
        let is_definitely_not_speech = self.speech_presence < Self::GATE_CLOSE_THRESHOLD;

        // Hold counter: keep gate open briefly after speech ends
        if is_speech {
            let hold = if is_high_confidence {
                Self::HOLD_FRAMES_EXTENDED
            } else {
                Self::HOLD_FRAMES
            };
            self.hold_counter = self.hold_counter.max(hold);
        } else if self.hold_counter > 0 {
            self.hold_counter -= 1;
        }

        // Target gain: open if speech detected or hold active
        let target_gain = if is_speech || self.hold_counter > 0 {
            Self::GATE_MAX_GAIN
        } else {
            Self::GATE_FLOOR
        };

        // Adaptive smoothing coefficient
        let coeff = if target_gain > self.gate_gain {
            // Opening — snap open faster if high confidence
            if is_high_confidence {
                0.65 // Very snappy
            } else {
                Self::GATE_ATTACK
            }
        } else {
            // Closing — fast when clearly not speech, slower in ambiguous zone
            if is_definitely_not_speech {
                Self::GATE_FAST_RELEASE
            } else {
                Self::GATE_RELEASE
            }
        };

        let delta = (target_gain - self.gate_gain) * coeff;
        if delta.abs() > 0.0001 {
            self.gate_gain += delta;
        } else if (target_gain - self.gate_gain).abs() > 0.0001 {
            self.gate_gain = target_gain;
        }

        self.gate_gain = self.gate_gain.clamp(Self::GATE_FLOOR, Self::GATE_MAX_GAIN);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wasapi_mic_config_default() {
        let config = WasapiMicConfig::default();
        assert_eq!(config.sample_rate, 48000);
        assert_eq!(config.channels, 2);
        assert_eq!(config.bits_per_sample, 16);
        assert_eq!(config.buffer_duration, Duration::from_millis(20));
        assert!(config.device_id.is_none());
        assert!(config.noise_reduction_enabled);
    }

    #[test]
    fn test_rnnoise_processor_latency() {
        let mut processor = RNNoiseProcessor::new(1);

        // Input 480 samples (10ms at 48kHz) of strong signal, twice
        let mut data: Vec<u8> = (0..960)
            .map(|i| ((i as f32 * 0.1).sin() * 20000.0) as i16)
            .flat_map(|s| s.to_ne_bytes())
            .collect();
        processor.process(&mut data);

        // Should NOT be all zeros because we provided > 1 frame
        let samples: &[i16] =
            unsafe { std::slice::from_raw_parts(data.as_ptr() as *const i16, data.len() / 2) };
        assert!(!samples.iter().all(|&s| s == 0));
    }

    #[test]
    fn test_rnnoise_stereo() {
        let mut processor = RNNoiseProcessor::new(2);

        // Provide 480 stereo frames
        let mut data = vec![0u8; 480 * 2 * 2];
        processor.process(&mut data);

        let samples: &[i16] =
            unsafe { std::slice::from_raw_parts(data.as_ptr() as *const i16, data.len() / 2) };
        assert_eq!(samples.len(), 960);
    }

    #[test]
    fn test_rnnoise_partial_frame_passthrough_is_not_silenced() {
        let mut processor = RNNoiseProcessor::new(1);

        let mut data: Vec<u8> = (0..200)
            .map(|i| (((i as f32) * 0.2).sin() * 12000.0) as i16)
            .flat_map(|s| s.to_ne_bytes())
            .collect();
        let original = data.clone();

        processor.process(&mut data);

        let samples: &[i16] =
            unsafe { std::slice::from_raw_parts(data.as_ptr() as *const i16, data.len() / 2) };
        let original_samples: &[i16] = unsafe {
            std::slice::from_raw_parts(original.as_ptr() as *const i16, original.len() / 2)
        };
        assert!(samples.iter().any(|&s| s != 0));
        assert!(samples
            .iter()
            .zip(original_samples.iter())
            .any(|(&processed, &dry)| processed != 0 && dry != 0));
    }

    #[test]
    fn test_rnnoise_stereo_broadcasts_mono() {
        let mut processor = RNNoiseProcessor::new(2);

        // Feed enough data to get past warmup (need > 480 mono frames = > 960 stereo samples)
        let mut data: Vec<u8> = Vec::with_capacity(960 * 2 * 2);
        for i in 0..960 {
            let v = ((i as f32 * 0.1).sin() * 15000.0) as i16;
            data.extend_from_slice(&v.to_ne_bytes());
            data.extend_from_slice(&v.to_ne_bytes());
        }

        processor.process(&mut data);

        // After RNNoise, L and R should be identical (mono broadcast)
        let samples: &[i16] =
            unsafe { std::slice::from_raw_parts(data.as_ptr() as *const i16, data.len() / 2) };
        // Check the second half (past warmup)
        for pair in samples[960..].chunks_exact(2) {
            assert_eq!(
                pair[0], pair[1],
                "L/R should be identical after mono broadcast"
            );
        }
    }

    #[test]
    fn test_rnnoise_output_reaches_final_audio() {
        // Regression test: verify that RNNoise output is NOT thrown away.
        // The old pending_wet_drop bug caused all denoised output to be discarded.
        let mut processor = RNNoiseProcessor::new(1);

        // Generate a strong signal across multiple packets (like real WASAPI 20ms packets)
        let make_packet = |amplitude: f32| -> Vec<u8> {
            (0..960)
                .map(|i| ((i as f32 * 0.1).sin() * amplitude) as i16)
                .flat_map(|s| s.to_ne_bytes())
                .collect()
        };

        // Process several packets to get past warmup
        for _ in 0..5 {
            let mut data = make_packet(20000.0);
            processor.process(&mut data);
        }

        // Now process one more packet and check that output differs from input
        // (RNNoise should have modified the signal)
        let mut data = make_packet(20000.0);
        let original = data.clone();
        processor.process(&mut data);

        let processed: &[i16] =
            unsafe { std::slice::from_raw_parts(data.as_ptr() as *const i16, data.len() / 2) };
        let original_samples: &[i16] = unsafe {
            std::slice::from_raw_parts(original.as_ptr() as *const i16, original.len() / 2)
        };

        // At least some samples should differ (RNNoise modifies the signal)
        let differs = processed
            .iter()
            .zip(original_samples.iter())
            .filter(|(&a, &b)| a != b)
            .count();
        assert!(
            differs > processed.len() / 4,
            "RNNoise output should modify most samples, but only {}/{} differed",
            differs,
            processed.len()
        );
    }

    #[test]
    fn test_adaptive_noise_gate_hysteresis() {
        let mut processor = RNNoiseProcessor::new(1);

        // Gate should start closed (at floor)
        assert!(processor.gate_gain < 0.1);

        // Simulate several frames of "speech" to open the gate
        for _ in 0..15 {
            let mut data: Vec<u8> = (0..AUDIO_FRAME_SIZE)
                .map(|i| ((i as f32 * 0.5).sin() * 20000.0) as i16)
                .flat_map(|s| s.to_ne_bytes())
                .collect();
            processor.process(&mut data);
        }

        let gain_after_speech = processor.gate_gain;
        assert!(
            gain_after_speech > 0.5,
            "Gate should open after sustained speech, got {}",
            gain_after_speech
        );

        assert!(
            processor.hold_counter > 0
                || processor.speech_presence > RNNoiseProcessor::GATE_CLOSE_THRESHOLD
        );
    }

    #[test]
    fn test_adaptive_noise_gate_stays_bounded() {
        let mut processor = RNNoiseProcessor::new(1);

        for _ in 0..50 {
            let mut data: Vec<u8> = (0..AUDIO_FRAME_SIZE)
                .map(|i| ((i as f32 * 0.3).sin() * 5000.0) as i16)
                .flat_map(|s| s.to_ne_bytes())
                .collect();
            processor.process(&mut data);
        }

        assert!(processor.gate_gain >= RNNoiseProcessor::GATE_FLOOR);
        assert!(processor.gate_gain <= RNNoiseProcessor::GATE_MAX_GAIN);
    }
}
