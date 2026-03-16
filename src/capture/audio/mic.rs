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
        if noise_tx.is_none() {
            tracing::info!("Microphone raw passthrough enabled (noise reduction disabled)");
        }

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
    quiet_packet_streak: u8,
    agc: AgcState,
}

struct AgcState {
    target_level: f32,
    peak_ceiling: f32,
    current_gain: f32,
    min_gain: f32,
    max_gain: f32,
    gain_up_alpha: f32,
    gain_down_alpha: f32,
    rms_alpha: f32,
    peak_alpha: f32,
    noise_floor_fast_alpha: f32,
    noise_floor_slow_alpha: f32,
    speech_attack_alpha: f32,
    speech_release_alpha: f32,
    smoothed_rms: f32,
    smoothed_peak: f32,
    noise_floor_rms: f32,
    speech_presence: f32,
}

impl RNNoiseProcessor {
    const DC_COEFF: f32 = 0.9975;
    const MIN_GAIN: f32 = 0.18;
    const QUIET_SPEECH_GAIN: f32 = 0.90;
    const MAX_GAIN: f32 = 1.60;
    const QUIET_SPEECH_MAKEUP_GAIN: f32 = 1.10;
    const VAD_NOISE_THRESHOLD: f32 = 0.22;
    const VAD_GATE_THRESHOLD: f32 = 0.52;
    const SNR_MIN: f32 = 1.15;
    const SNR_MAX: f32 = 5.5;
    const HANGOVER_FRAMES: u8 = 24;
    const NOISE_FLOOR_FAST_ALPHA: f32 = 0.10;
    const NOISE_FLOOR_SLOW_ALPHA: f32 = 0.01;
    const QUIET_PACKET_STREAK_THRESHOLD: u8 = 4;
    const QUIET_PACKET_MAX_MEAN_ABS: f32 = 48.0;
    const QUIET_PACKET_MAX_PEAK: i32 = 900;
    const QUIET_BYPASS_MAX_SPEECH_PRESENCE: f32 = 0.08;

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
            quiet_packet_streak: 0,
            agc: AgcState::new(),
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
        let base_gain = Self::lerp(adaptive_floor, 1.0, openness);
        let quiet_speech_makeup = (1.0 - openness) * floor_shape * Self::QUIET_SPEECH_MAKEUP_GAIN;
        (base_gain + quiet_speech_makeup).clamp(Self::MIN_GAIN, Self::MAX_GAIN)
    }

    #[inline]
    fn reset_after_quiet_bypass(&mut self) {
        self.in_buf.clear();
        self.in_buf.resize(AUDIO_FRAME_SIZE, 0.0);
        self.in_head = 0;
        self.out_buf.clear();
        self.out_head = 0;
        self.gain = Self::MIN_GAIN;
        self.speech_presence = 0.0;
        self.speech_hangover = 0;
        self.primed = false;
    }

    #[inline]
    fn maybe_bypass_quiet_packet(&mut self, data: &mut [u8]) -> bool {
        if self.channels == 0 || data.len() % (self.channels * 2) != 0 {
            return false;
        }

        let sample_count = data.len() / 2;
        let frame_count = sample_count / self.channels;
        if frame_count == 0 {
            return false;
        }

        let samples =
            unsafe { std::slice::from_raw_parts_mut(data.as_mut_ptr() as *mut i16, sample_count) };

        let (sum_abs, peak_abs) = match self.channels {
            1 => samples.iter().fold((0i64, 0i32), |(sum, peak), &sample| {
                let abs = (sample as i32).abs();
                (sum + abs as i64, peak.max(abs))
            }),
            2 => samples
                .chunks_exact(2)
                .fold((0i64, 0i32), |(sum, peak), frame| {
                    let mono = ((frame[0] as i32) + (frame[1] as i32)) / 2;
                    let abs = mono.abs();
                    (sum + abs as i64, peak.max(abs))
                }),
            _ => samples
                .chunks_exact(self.channels)
                .fold((0i64, 0i32), |(sum, peak), frame| {
                    let mono = frame.iter().map(|&sample| sample as i32).sum::<i32>()
                        / self.channels as i32;
                    let abs = mono.abs();
                    (sum + abs as i64, peak.max(abs))
                }),
        };

        let mean_abs = sum_abs as f32 / frame_count as f32;
        let quiet_packet = self.speech_hangover == 0
            && self.speech_presence <= Self::QUIET_BYPASS_MAX_SPEECH_PRESENCE
            && mean_abs <= Self::QUIET_PACKET_MAX_MEAN_ABS
            && peak_abs <= Self::QUIET_PACKET_MAX_PEAK;

        if quiet_packet {
            self.quiet_packet_streak = self.quiet_packet_streak.saturating_add(1);
        } else {
            self.quiet_packet_streak = 0;
            return false;
        }

        if self.quiet_packet_streak < Self::QUIET_PACKET_STREAK_THRESHOLD {
            return false;
        }

        data.fill(0);
        self.reset_after_quiet_bypass();
        true
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

        if self.maybe_bypass_quiet_packet(data) {
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
                for (dst, frame) in self.in_buf[write_start..]
                    .iter_mut()
                    .zip(samples.chunks_exact(2))
                {
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
        self.out_buf
            .reserve(complete_frames.saturating_mul(AUDIO_FRAME_SIZE));

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
            self.speech_presence += presence_alpha * (presence_target - self.speech_presence);

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

        // Apply AGC to the processed output buffer
        let agc_start = self.out_head;
        let agc_end = self.out_buf.len().min(self.out_head + frame_count);
        if agc_end > agc_start {
            self.agc
                .process_frame(&mut self.out_buf[agc_start..agc_end]);
        }

        // Broadcast processed mono output to all interleaved channels,
        // with per-channel DC blocking and soft limiting.
        let available = (self.out_buf.len() - self.out_head).min(frame_count);
        match self.channels {
            1 => {
                for frame_idx in 0..available {
                    let mono_sample = self.out_buf[self.out_head + frame_idx];
                    samples[frame_idx] = soft_limit(self.dc_block_broadcast(mono_sample))
                        .clamp(-32768.0, 32767.0) as i16;
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

impl AgcState {
    const TARGET_LEVEL_DB: f32 = -18.0;
    const PEAK_CEILING_DB: f32 = -9.0;
    const MIN_GAIN_DB: f32 = -6.0;
    const MAX_GAIN_DB: f32 = 18.0;
    const GAIN_UP_MS: f32 = 120.0;
    const GAIN_DOWN_MS: f32 = 45.0;
    const RMS_SMOOTHING_MS: f32 = 140.0;
    const PEAK_SMOOTHING_MS: f32 = 45.0;
    const NOISE_FLOOR_FAST_MS: f32 = 180.0;
    const NOISE_FLOOR_SLOW_MS: f32 = 2400.0;
    const SPEECH_ATTACK_MS: f32 = 70.0;
    const SPEECH_RELEASE_MS: f32 = 260.0;
    const MIN_ACTIVE_RMS: f32 = 120.0;
    const MIN_NOISE_FLOOR_RMS: f32 = 48.0;
    const SPEECH_SNR_MIN: f32 = 1.8;
    const SPEECH_SNR_MAX: f32 = 10.0;

    fn new() -> Self {
        let target_level = 10.0f32.powf(Self::TARGET_LEVEL_DB / 20.0) * 32768.0;
        let peak_ceiling = 10.0f32.powf(Self::PEAK_CEILING_DB / 20.0) * 32768.0;
        let min_gain = 10.0f32.powf(Self::MIN_GAIN_DB / 20.0);
        let max_gain = 10.0f32.powf(Self::MAX_GAIN_DB / 20.0);

        Self {
            target_level,
            peak_ceiling,
            current_gain: 1.0,
            min_gain,
            max_gain,
            gain_up_alpha: Self::frame_alpha_from_ms(Self::GAIN_UP_MS),
            gain_down_alpha: Self::frame_alpha_from_ms(Self::GAIN_DOWN_MS),
            rms_alpha: Self::frame_alpha_from_ms(Self::RMS_SMOOTHING_MS),
            peak_alpha: Self::frame_alpha_from_ms(Self::PEAK_SMOOTHING_MS),
            noise_floor_fast_alpha: Self::frame_alpha_from_ms(Self::NOISE_FLOOR_FAST_MS),
            noise_floor_slow_alpha: Self::frame_alpha_from_ms(Self::NOISE_FLOOR_SLOW_MS),
            speech_attack_alpha: Self::frame_alpha_from_ms(Self::SPEECH_ATTACK_MS),
            speech_release_alpha: Self::frame_alpha_from_ms(Self::SPEECH_RELEASE_MS),
            smoothed_rms: 0.0,
            smoothed_peak: 0.0,
            noise_floor_rms: Self::MIN_NOISE_FLOOR_RMS,
            speech_presence: 0.0,
        }
    }

    #[inline]
    fn frame_alpha_from_ms(ms: f32) -> f32 {
        if ms <= 0.0 {
            return 1.0;
        }

        let frame_rate = 100.0;
        let tau_seconds = ms / 1000.0;
        (1.0 - (-1.0 / (frame_rate * tau_seconds)).exp()).clamp(0.000001, 1.0)
    }

    #[inline]
    fn frame_rms_and_peak(frame: &[f32]) -> (f32, f32) {
        let mut sum_sq = 0.0f32;
        let mut peak = 0.0f32;
        for &sample in frame {
            sum_sq += sample * sample;
            peak = peak.max(sample.abs());
        }

        let rms = if frame.is_empty() {
            0.0
        } else {
            (sum_sq / frame.len() as f32).sqrt()
        };

        (rms, peak)
    }

    #[inline]
    fn update_speech_presence(&mut self, instant_speech: f32) {
        let alpha = if instant_speech >= self.speech_presence {
            self.speech_attack_alpha
        } else {
            self.speech_release_alpha
        };
        self.speech_presence += alpha * (instant_speech - self.speech_presence);
    }

    #[inline]
    fn compute_instant_speech_factor(&self, frame_rms: f32, frame_peak: f32) -> f32 {
        if frame_peak <= Self::MIN_ACTIVE_RMS * 0.5 || frame_rms <= Self::MIN_ACTIVE_RMS * 0.35 {
            return 0.0;
        }

        let snr = frame_rms / (self.noise_floor_rms + 1.0);
        let snr_factor =
            ((snr - Self::SPEECH_SNR_MIN) / (Self::SPEECH_SNR_MAX - Self::SPEECH_SNR_MIN))
                .clamp(0.0, 1.0);
        let level_factor = ((frame_rms - Self::MIN_ACTIVE_RMS) / (self.target_level - Self::MIN_ACTIVE_RMS))
            .clamp(0.0, 1.0);
        let peak_factor = ((frame_peak - Self::MIN_ACTIVE_RMS) / (self.target_level * 1.35 - Self::MIN_ACTIVE_RMS))
            .clamp(0.0, 1.0);
        (snr_factor * 0.55 + level_factor * 0.25 + peak_factor * 0.20).clamp(0.0, 1.0)
    }

    #[inline]
    fn compute_target_gain(&self) -> f32 {
        if self.speech_presence <= 0.02 || self.smoothed_rms <= Self::MIN_ACTIVE_RMS {
            return 1.0;
        }

        let rms_gain = self.target_level / self.smoothed_rms.max(Self::MIN_ACTIVE_RMS);
        let peak_limited_gain = if self.smoothed_peak > 1.0 {
            self.peak_ceiling / self.smoothed_peak
        } else {
            self.max_gain
        };
        let unclamped_target = rms_gain.min(peak_limited_gain).clamp(self.min_gain, self.max_gain);
        let speech_weighted_gain = if unclamped_target >= 1.0 {
            let activation = (0.45 + self.speech_presence * 0.55).clamp(0.0, 1.0);
            1.0 + (unclamped_target - 1.0) * activation
        } else {
            unclamped_target
        };
        speech_weighted_gain.clamp(self.min_gain, self.max_gain)
    }

    #[inline]
    fn process_frame(&mut self, frame: &mut [f32]) {
        let (frame_rms, frame_peak) = Self::frame_rms_and_peak(frame);

        self.smoothed_rms += self.rms_alpha * (frame_rms - self.smoothed_rms);
        self.smoothed_peak += self.peak_alpha * (frame_peak - self.smoothed_peak);

        let instant_speech = self.compute_instant_speech_factor(frame_rms, frame_peak);
        self.update_speech_presence(instant_speech);

        let noise_floor_target = if instant_speech >= 0.15 || self.speech_presence >= 0.15 {
            self.noise_floor_rms.min(frame_rms)
        } else {
            frame_rms
        };
        let noise_floor_alpha = if noise_floor_target <= self.noise_floor_rms * 1.2 {
            self.noise_floor_fast_alpha
        } else {
            self.noise_floor_slow_alpha
        };
        self.noise_floor_rms += noise_floor_alpha * (noise_floor_target - self.noise_floor_rms);
        self.noise_floor_rms = self
            .noise_floor_rms
            .clamp(Self::MIN_NOISE_FLOOR_RMS, self.target_level.max(Self::MIN_NOISE_FLOOR_RMS));

        let target_gain = self.compute_target_gain();
        let alpha = if target_gain >= self.current_gain {
            self.gain_up_alpha
        } else {
            self.gain_down_alpha
        };
        self.current_gain += alpha * (target_gain - self.current_gain);

        for sample in frame.iter_mut() {
            *sample *= self.current_gain;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_frame_rms(frame: &[f32]) -> f32 {
        let sum_sq = frame.iter().map(|sample| sample * sample).sum::<f32>();
        (sum_sq / frame.len() as f32).sqrt()
    }

    fn test_frame_peak(frame: &[f32]) -> f32 {
        frame.iter().fold(0.0f32, |peak, sample| peak.max(sample.abs()))
    }

    fn synth_sine_frame(amplitude: f32, frame_index: usize) -> Vec<f32> {
        (0..AUDIO_FRAME_SIZE)
            .map(|i| {
                let phase = frame_index as f32 * 0.37 + i as f32 * 0.11;
                phase.sin() * amplitude
            })
            .collect()
    }

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
    fn test_quiet_packet_bypass_zeros_after_streak() {
        let mut processor = RNNoiseProcessor::new(2);
        let mut data = vec![0u8; 480 * 2 * 2];

        for _ in 0..RNNoiseProcessor::QUIET_PACKET_STREAK_THRESHOLD {
            processor.process(&mut data);
        }

        let samples: &[i16] =
            unsafe { std::slice::from_raw_parts(data.as_ptr() as *const i16, data.len() / 2) };
        assert!(samples.iter().all(|&s| s == 0));
    }

    #[test]
    fn test_adaptive_target_gain_preserves_quiet_speech() {
        let noise_only = RNNoiseProcessor::compute_adaptive_target_gain(0.05, 0.0, 0.0);
        let quiet_speech = RNNoiseProcessor::compute_adaptive_target_gain(0.32, 0.18, 0.55);

        assert!(quiet_speech > noise_only + 0.60);
        assert!(quiet_speech > 1.0);
        assert!(quiet_speech <= RNNoiseProcessor::MAX_GAIN);
    }

    #[test]
    fn test_adaptive_target_gain_stays_bounded() {
        let gain = RNNoiseProcessor::compute_adaptive_target_gain(0.95, 1.0, 1.0);
        assert!(gain >= RNNoiseProcessor::MIN_GAIN);
        assert!(gain <= RNNoiseProcessor::MAX_GAIN);
    }

    #[test]
    fn test_agc_boosts_quiet_speech_toward_voice_chat_level() {
        let mut agc = AgcState::new();
        let input_rms = test_frame_rms(&synth_sine_frame(900.0, 0));
        let mut output_rms = input_rms;

        for frame_index in 0..220 {
            let mut frame = synth_sine_frame(900.0, frame_index);
            agc.process_frame(&mut frame);
            output_rms = test_frame_rms(&frame);
        }

        assert!(
            output_rms > input_rms * 2.2,
            "quiet speech output_rms={} input_rms={} gain={} speech_presence={}",
            output_rms,
            input_rms,
            agc.current_gain,
            agc.speech_presence
        );
        assert!(
            output_rms > agc.target_level * 0.55,
            "quiet speech output_rms={} target_level={} gain={} speech_presence={}",
            output_rms,
            agc.target_level,
            agc.current_gain,
            agc.speech_presence
        );
        assert!(output_rms < agc.peak_ceiling);
        assert!(agc.current_gain > 1.8);
    }

    #[test]
    fn test_agc_does_not_pump_noise_floor() {
        let mut agc = AgcState::new();
        let mut output_rms = 0.0;

        for frame_index in 0..260 {
            let mut frame = synth_sine_frame(36.0, frame_index);
            agc.process_frame(&mut frame);
            output_rms = test_frame_rms(&frame);
        }

        assert!(agc.current_gain < 1.15);
        assert!(output_rms < 48.0);
        assert!(agc.speech_presence < 0.1);
    }

    #[test]
    fn test_agc_attenuates_loud_speech_below_peak_ceiling() {
        let mut agc = AgcState::new();
        let mut output_peak = 0.0;

        for frame_index in 0..140 {
            let mut frame = synth_sine_frame(22_000.0, frame_index);
            agc.process_frame(&mut frame);
            output_peak = test_frame_peak(&frame);
        }

        assert!(agc.current_gain < 0.85);
        assert!(output_peak <= agc.peak_ceiling * 1.08);
    }
}
