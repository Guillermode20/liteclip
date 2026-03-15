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
            buffer_duration: Duration::from_millis(100),
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

        let mut total_frames: u64 = 0;
        let mut capture_discontinuities: u64 = 0;
        let mut timestamp_errors: u64 = 0;

        let max_buffer_size = (config.sample_rate as usize / 10) * block_align as usize;
        let mut audio_buffer = BytesMut::with_capacity(max_buffer_size);

        while running.load(Ordering::SeqCst) {
            let mut packet_frames = unsafe { capture_client.GetNextPacketSize() }?;

            if packet_frames == 0 {
                match unsafe { WaitForSingleObject(capture_event.raw(), 100) }.0 {
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
                    if capture_discontinuities == 1 || capture_discontinuities % 100 == 0 {
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
                if last_log.elapsed() >= Duration::from_secs(2) {
                    tracing::info!("RNNoise: processed {} packets in 2s", packets);
                    packets = 0;
                    last_log = std::time::Instant::now();
                }
                let frozen = frame.data.freeze();
                let packet = EncodedPacket::new(
                    frozen,
                    frame.pts,
                    frame.pts,
                    false,
                    StreamType::Microphone,
                );
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

fn soft_limit(x: f32) -> f32 {
    const THRESHOLD: f32 = 16_384.0;
    const MAX: f32 = 32_767.0;

    if x.abs() < THRESHOLD {
        x
    } else {
        let sign = x.signum();
        let over = (x.abs() - THRESHOLD) / (MAX - THRESHOLD);
        sign * (THRESHOLD + (MAX - THRESHOLD) * over.tanh())
    }
}

struct RNNoiseProcessor {
    channels: usize,
    state: Box<DenoiseState<'static>>,
    in_buf: Vec<f32>,
    in_head: usize,
    out_buf: Vec<f32>,
    out_head: usize,
    gain: f32,
    speech_presence: f32,
    noise_floor: f32,
    speech_hangover: u8,
    dc_x: Vec<f32>,
    dc_y: Vec<f32>,
    frame_in: Box<[f32; AUDIO_FRAME_SIZE]>,
    frame_out: Box<[f32; AUDIO_FRAME_SIZE]>,
    primed: bool,
    attack_alpha: f32,
    release_alpha: f32,
    presence_attack_alpha: f32,
    presence_release_alpha: f32,
}

impl RNNoiseProcessor {
    const DC_COEFF: f32 = 0.9975;
    const MIN_GAIN: f32 = 0.18;
    const QUIET_SPEECH_GAIN: f32 = 0.62;
    const VAD_NOISE_THRESHOLD: f32 = 0.22;
    const VAD_GATE_THRESHOLD: f32 = 0.52;
    const SNR_MIN: f32 = 1.15;
    const SNR_MAX: f32 = 5.5;
    const HANGOVER_FRAMES: u8 = 24;
    const NOISE_FLOOR_FAST_ALPHA: f32 = 0.10;
    const NOISE_FLOOR_SLOW_ALPHA: f32 = 0.01;

    fn new(channels: usize) -> Self {
        let mut in_buf = Vec::with_capacity(AUDIO_FRAME_SIZE * 16);
        in_buf.resize(AUDIO_FRAME_SIZE, 0.0);
        let out_buf = Vec::with_capacity(AUDIO_FRAME_SIZE * 16);
        Self {
            channels,
            state: DenoiseState::new(),
            in_buf,
            in_head: 0,
            out_buf,
            out_head: 0,
            gain: Self::MIN_GAIN,
            speech_presence: 0.0,
            noise_floor: 300.0,
            speech_hangover: 0,
            dc_x: vec![0.0; channels],
            dc_y: vec![0.0; channels],
            frame_in: Box::new([0.0; AUDIO_FRAME_SIZE]),
            frame_out: Box::new([0.0; AUDIO_FRAME_SIZE]),
            primed: false,
            attack_alpha: Self::alpha_from_ms(1.5),
            release_alpha: Self::alpha_from_ms(80.0),
            presence_attack_alpha: Self::frame_alpha_from_ms(30.0),
            presence_release_alpha: Self::frame_alpha_from_ms(220.0),
        }
    }

    #[inline]
    fn alpha_from_ms(ms: f32) -> f32 {
        if ms <= 0.0 {
            return 1.0;
        }

        let sample_rate = 48_000.0;
        let tau_seconds = ms / 1000.0;
        let alpha = 1.0 - (-1.0 / (sample_rate * tau_seconds)).exp();
        alpha.clamp(0.000001, 1.0)
    }

    #[inline]
    fn frame_alpha_from_ms(ms: f32) -> f32 {
        if ms <= 0.0 {
            return 1.0;
        }

        let frame_rate = 100.0;
        let tau_seconds = ms / 1000.0;
        let alpha = 1.0 - (-1.0 / (frame_rate * tau_seconds)).exp();
        alpha.clamp(0.000001, 1.0)
    }

    #[inline]
    fn lerp(a: f32, b: f32, t: f32) -> f32 {
        a + (b - a) * t.clamp(0.0, 1.0)
    }

    #[inline]
    fn compute_adaptive_target_gain(vad: f32, snr_gate: f32, speech_presence: f32) -> f32 {
        let quiet_voice = ((vad - Self::VAD_NOISE_THRESHOLD)
            / (Self::VAD_GATE_THRESHOLD - Self::VAD_NOISE_THRESHOLD))
            .clamp(0.0, 1.0);
        let floor_shape = (speech_presence * 0.75 + quiet_voice * 0.25).clamp(0.0, 1.0);
        let adaptive_floor = Self::lerp(Self::MIN_GAIN, Self::QUIET_SPEECH_GAIN, floor_shape);
        let openness = (vad * 0.45 + snr_gate * 0.20 + speech_presence * 0.35).clamp(0.0, 1.0);
        Self::lerp(adaptive_floor, 1.0, openness)
    }

    #[inline]
    fn dc_block(&mut self, channel: usize, sample: f32) -> f32 {
        let filtered = sample - self.dc_x[channel] + Self::DC_COEFF * self.dc_y[channel];
        self.dc_x[channel] = sample;
        self.dc_y[channel] = filtered;
        filtered
    }

    #[inline]
    fn compact_queue(buf: &mut Vec<f32>, head: &mut usize) {
        if *head > 0 && *head >= buf.len() / 2 {
            let remaining = buf.len() - *head;
            buf.copy_within(*head.., 0);
            buf.truncate(remaining);
            *head = 0;
        }
    }

    #[inline]
    fn dc_block_broadcast(&mut self, sample: f32) -> f32 {
        let filtered = self.dc_block(0, sample);
        for channel in 1..self.channels {
            self.dc_x[channel] = sample;
            self.dc_y[channel] = filtered;
        }
        filtered
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

        // Mix interleaved input down to mono.  For a typical single-capsule
        // mic L == R, so this is lossless; it also halves RNNoise call count.
        let write_start = self.in_buf.len();
        self.in_buf.resize(write_start + frame_count, 0.0);
        match self.channels {
            1 => {
                for (dst, &sample) in self.in_buf[write_start..].iter_mut().zip(samples.iter()) {
                    *dst = sample as f32;
                }
            }
            2 => {
                for (dst, frame) in self.in_buf[write_start..].iter_mut().zip(samples.chunks_exact(2)) {
                    *dst = (frame[0] as f32 + frame[1] as f32) * 0.5;
                }
            }
            _ => {
                let channels_f = self.channels as f32;
                for (dst, frame) in self.in_buf[write_start..]
                    .iter_mut()
                    .zip(samples.chunks_exact(self.channels))
                {
                    let mut sum = 0.0f32;
                    for &sample in frame {
                        sum += sample as f32;
                    }
                    *dst = sum / channels_f;
                }
            }
        }

        let complete_frames = (self.in_buf.len() - self.in_head) / AUDIO_FRAME_SIZE;
        self.out_buf.reserve(complete_frames.saturating_mul(AUDIO_FRAME_SIZE));

        // Process complete 480-sample RNNoise frames from the mono queue.
        while (self.in_buf.len() - self.in_head) >= AUDIO_FRAME_SIZE {
            let frame_start = self.in_head;
            self.frame_in
                .copy_from_slice(&self.in_buf[frame_start..frame_start + AUDIO_FRAME_SIZE]);

            let vad = self
                .state
                .process_frame(self.frame_out.as_mut_slice(), self.frame_in.as_slice());

            let mut energy = 0.0f32;
            for &sample in self.frame_out.iter() {
                energy += sample * sample;
            }
            let frame_rms = (energy / AUDIO_FRAME_SIZE as f32).sqrt();

            let floor_alpha = if vad < Self::VAD_NOISE_THRESHOLD {
                Self::NOISE_FLOOR_FAST_ALPHA
            } else {
                Self::NOISE_FLOOR_SLOW_ALPHA
            };
            self.noise_floor += floor_alpha * (frame_rms - self.noise_floor);
            self.noise_floor = self.noise_floor.max(10.0);

            let snr = frame_rms / (self.noise_floor + 1.0);
            let snr_gate =
                ((snr - Self::SNR_MIN) / (Self::SNR_MAX - Self::SNR_MIN)).clamp(0.0, 1.0);

            let instant_speech = (vad * 0.72 + snr_gate * 0.28).clamp(0.0, 1.0);
            if vad >= Self::VAD_GATE_THRESHOLD {
                self.speech_hangover = Self::HANGOVER_FRAMES;
            }

            let mut presence_target = instant_speech;
            if self.speech_hangover > 0 {
                self.speech_hangover -= 1;
                let hold = (self.speech_hangover as f32 / Self::HANGOVER_FRAMES as f32).max(0.35);
                presence_target = presence_target.max(hold);
            }

            let presence_alpha = if presence_target >= self.speech_presence {
                self.presence_attack_alpha
            } else {
                self.presence_release_alpha
            };
            self.speech_presence +=
                presence_alpha * (presence_target - self.speech_presence);

            let target_gain =
                Self::compute_adaptive_target_gain(vad, snr_gate, self.speech_presence);
            let mut current_gain = self.gain;

            let out_start = self.out_buf.len();
            self.out_buf.resize(out_start + AUDIO_FRAME_SIZE, 0.0);

            let gain_alpha = if target_gain >= current_gain {
                self.attack_alpha
            } else {
                self.release_alpha
            };
            for i in 0..AUDIO_FRAME_SIZE {
                current_gain += gain_alpha * (target_gain - current_gain);
                self.out_buf[out_start + i] = self.frame_out[i] * current_gain;
            }

            self.gain = current_gain;
            self.primed = true;
            self.in_head += AUDIO_FRAME_SIZE;
            Self::compact_queue(&mut self.in_buf, &mut self.in_head);
        }

        // Broadcast processed mono output to all interleaved channels,
        // with per-channel DC blocking and soft limiting.
        let available = (self.out_buf.len() - self.out_head).min(frame_count);
        match self.channels {
            1 => {
                for frame_idx in 0..available {
                    let mono_sample = self.out_buf[self.out_head + frame_idx];
                    samples[frame_idx] =
                        soft_limit(self.dc_block_broadcast(mono_sample)).clamp(-32768.0, 32767.0)
                            as i16;
                }
            }
            2 => {
                for frame_idx in 0..available {
                    let mono_sample = self.out_buf[self.out_head + frame_idx];
                    let limited = soft_limit(self.dc_block_broadcast(mono_sample))
                        .clamp(-32768.0, 32767.0) as i16;
                    let sample_idx = frame_idx * 2;
                    samples[sample_idx] = limited;
                    samples[sample_idx + 1] = limited;
                }
            }
            _ => {
                for frame_idx in 0..available {
                    let mono_sample = self.out_buf[self.out_head + frame_idx];
                    let limited = soft_limit(self.dc_block_broadcast(mono_sample))
                        .clamp(-32768.0, 32767.0) as i16;
                    let sample_idx = frame_idx * self.channels;
                    for channel in 0..self.channels {
                        samples[sample_idx + channel] = limited;
                    }
                }
            }
        }

        self.out_head += available;
        Self::compact_queue(&mut self.out_buf, &mut self.out_head);

        // For any frames the output buffer couldn't cover, leave the original
        // WASAPI PCM in place (no noise reduction for those samples).  Injecting
        // zeros would create a hard discontinuity → crackling; un-reduced audio
        // is inaudible for the brief (<1 ms) moments this can occur.
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
        assert_eq!(config.buffer_duration, Duration::from_millis(100));
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
    fn test_adaptive_target_gain_preserves_quiet_speech() {
        let noise_only = RNNoiseProcessor::compute_adaptive_target_gain(0.05, 0.0, 0.0);
        let quiet_speech = RNNoiseProcessor::compute_adaptive_target_gain(0.32, 0.18, 0.55);

        assert!(quiet_speech > noise_only + 0.25);
        assert!(quiet_speech <= 1.0);
    }

    #[test]
    fn test_adaptive_target_gain_stays_bounded() {
        let gain = RNNoiseProcessor::compute_adaptive_target_gain(0.95, 1.0, 1.0);
        assert!(gain >= RNNoiseProcessor::MIN_GAIN);
        assert!(gain <= 1.0);
    }
}
