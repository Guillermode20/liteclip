//! WASAPI Microphone Capture
//!
//! Captures microphone audio using WASAPI in shared mode.

use anyhow::{Context, Result};
use bytes::BytesMut;
use crossbeam::channel::{bounded, Receiver, Sender};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tracing::{debug, error, warn};

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Media::Audio::{
    eCapture, eConsole, IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator, MMDeviceEnumerator,
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
    AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, WAVEFORMATEX,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED,
};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

use crate::buffer::ring::qpc_frequency;
use crate::encode::{EncodedPacket, StreamType};

/// Runtime-tunable settings for microphone noise suppression.
#[derive(Debug, Clone)]
pub struct NoiseSuppressorSettings {
    pub gate_threshold_db: f32,
    pub gate_hysteresis_db: f32,
    pub attack_ms: f32,
    pub release_ms: f32,
    pub hold_ms: f32,
    pub min_gain_db: f32,
}

impl Default for NoiseSuppressorSettings {
    fn default() -> Self {
        Self {
            gate_threshold_db: -45.0,
            gate_hysteresis_db: 4.0,
            attack_ms: 2.0,
            release_ms: 150.0,
            hold_ms: 100.0,
            min_gain_db: -60.0,
        }
    }
}

impl NoiseSuppressorSettings {
    fn sanitize(&mut self) {
        self.gate_threshold_db = self.gate_threshold_db.clamp(-90.0, -10.0);
        self.gate_hysteresis_db = self.gate_hysteresis_db.clamp(0.0, 20.0);
        self.attack_ms = self.attack_ms.clamp(0.1, 100.0);
        self.release_ms = self.release_ms.clamp(1.0, 2000.0);
        self.hold_ms = self.hold_ms.clamp(0.0, 1000.0);
        self.min_gain_db = self.min_gain_db.clamp(-120.0, 0.0);
    }
}

/// Configuration for WASAPI microphone capture
#[derive(Debug, Clone)]
pub struct WasapiMicConfig {
    pub sample_rate: u32,
    pub channels: u16,
    pub bits_per_sample: u16,
    pub buffer_duration: Duration,
    pub device_id: Option<String>, // None for default device
    pub noise_reduction_enabled: bool,
    pub noise_suppressor_settings: NoiseSuppressorSettings,
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
            noise_suppressor_settings: NoiseSuppressorSettings::default(),
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
        let (packet_tx, packet_rx) = bounded(64); // Buffer for audio packets

        Ok(Self {
            running: Arc::new(AtomicBool::new(false)),
            initialized: Arc::new(AtomicBool::new(false)),
            packet_tx,
            packet_rx,
            processed_samples: Arc::new(AtomicU64::new(0)),
            capture_thread: None,
        })
    }

    /// Start capturing microphone audio
    pub fn start(&mut self, config: WasapiMicConfig) -> Result<()> {
        if self.running.load(Ordering::SeqCst) {
            return Ok(());
        }

        self.running.store(true, Ordering::SeqCst);
        self.initialized.store(false, Ordering::SeqCst);

        let running = Arc::clone(&self.running);
        let initialized = Arc::clone(&self.initialized);
        let packet_tx = self.packet_tx.clone();
        let processed_samples = Arc::clone(&self.processed_samples);

        // Spawn the capture thread
        self.capture_thread = Some(thread::spawn(move || {
            if let Err(e) =
                Self::capture_loop(running, initialized, packet_tx, processed_samples, config)
            {
                error!("Microphone audio capture error: {}", e);
            }
        }));

        // Wait briefly for the capture thread to initialize WASAPI and report status without
        // always paying a fixed half-second startup penalty.
        for _ in 0..50 {
            if self.initialized.load(Ordering::SeqCst) || !self.running.load(Ordering::SeqCst) {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }

        if self.initialized.load(Ordering::SeqCst) {
            debug!("WASAPI microphone audio capture started and initialized successfully");
        } else if self.running.load(Ordering::SeqCst) {
            warn!("WASAPI microphone capture thread has not confirmed initialization after 500ms; mic audio may be unavailable");
        } else {
            warn!("WASAPI microphone capture thread exited during initialization; mic audio is unavailable");
        }

        Ok(())
    }

    /// Stop capturing microphone audio
    pub fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(handle) = self.capture_thread.take() {
            if handle.join().is_err() {
                error!("Microphone audio capture thread panicked");
            }
        }
        debug!("WASAPI microphone audio capture stopped");
    }

    /// Get receiver for captured audio packets
    pub fn packet_rx(&self) -> Receiver<EncodedPacket> {
        self.packet_rx.clone()
    }

    /// Main capture loop
    fn capture_loop(
        running: Arc<AtomicBool>,
        initialized: Arc<AtomicBool>,
        packet_tx: Sender<EncodedPacket>,
        processed_samples: Arc<AtomicU64>,
        config: WasapiMicConfig,
    ) -> Result<()> {
        debug!("Starting WASAPI microphone capture loop");

        let _com = ComApartment::initialize()?;

        Self::set_audio_thread_priority();

        if config.device_id.is_some() {
            warn!("Microphone custom device_id is not implemented yet; using default capture endpoint");
        }

        let enumerator: IMMDeviceEnumerator =
            unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
                .context("Failed to create MMDeviceEnumerator")?;

        // Log all available capture devices so the user can see what's available
        crate::capture::audio::device_info::log_all_capture_devices(&enumerator);

        let device = unsafe { enumerator.GetDefaultAudioEndpoint(eCapture, eConsole) }
            .context("Failed to get default microphone endpoint")?;

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
        let mut total_frames: u64 = 0;

        let mut noise_processor = if config.sample_rate == 48000
            && config.bits_per_sample == 16
            && config.noise_reduction_enabled
        {
            Some(MicNoiseProcessor::SmarterNoiseGate(SmarterNoiseGate::new(
                config.channels as usize,
                config.noise_suppressor_settings,
            )))
        } else {
            None
        };

        let max_buffer_size = (config.sample_rate as usize / 10) * block_align as usize;
        let mut audio_buffer = BytesMut::with_capacity(max_buffer_size);

        while running.load(Ordering::SeqCst) {
            let mut packet_frames = unsafe { capture_client.GetNextPacketSize() }
                .context("IAudioCaptureClient::GetNextPacketSize failed")?;

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
                unsafe {
                    if flags & (AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) == 0 && !data_ptr.is_null() {
                        std::ptr::copy_nonoverlapping(
                            data_ptr,
                            audio_buffer.as_mut_ptr(),
                            byte_count,
                        );
                    }
                }

                unsafe { capture_client.ReleaseBuffer(frame_count) }
                    .context("IAudioCaptureClient::ReleaseBuffer failed")?;

                if let Some(processor) = &mut noise_processor {
                    processor.process(&mut audio_buffer);
                }

                let pts = if qpc_position > 0 {
                    qpc_position.min(i64::MAX as u64) as i64
                } else {
                    start_qpc + ((total_frames as f64 / sample_rate) * qpc_freq) as i64
                };
                total_frames = total_frames.saturating_add(frame_count as u64);

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

                processed_samples.fetch_add(frame_count as u64, Ordering::SeqCst);

                packet_frames = unsafe { capture_client.GetNextPacketSize() }
                    .context("IAudioCaptureClient::GetNextPacketSize failed")?;
            }
        }

        unsafe { audio_client.Stop() }.context("Failed to stop microphone capture")?;

        debug!("WASAPI microphone audio capture loop ended");
        Ok(())
    }
}

struct EventHandle(HANDLE);

impl EventHandle {
    fn new() -> Result<Self> {
        let handle =
            unsafe { CreateEventW(None, false, false, None) }.context("CreateEventW failed")?;
        Ok(Self(handle))
    }

    fn raw(&self) -> HANDLE {
        self.0
    }
}

impl Drop for EventHandle {
    fn drop(&mut self) {
        if self.0 != HANDLE::default() {
            let _ = unsafe { CloseHandle(self.0) };
        }
    }
}

fn duration_to_hns(duration: Duration) -> i64 {
    (duration.as_secs_f64() * 10_000_000.0) as i64
}

fn query_qpc() -> Result<i64> {
    let mut qpc = 0i64;
    unsafe { windows::Win32::System::Performance::QueryPerformanceCounter(&mut qpc) }
        .context("QueryPerformanceCounter failed")?;
    Ok(qpc)
}

struct ComApartment;

impl ComApartment {
    fn initialize() -> Result<Self> {
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok() }
            .context("CoInitializeEx failed for WASAPI microphone capture")?;
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

enum MicNoiseProcessor {
    SmarterNoiseGate(SmarterNoiseGate),
}

impl MicNoiseProcessor {
    fn process(&mut self, data: &mut [u8]) {
        match self {
            Self::SmarterNoiseGate(processor) => processor.process(data),
        }
    }
}

#[inline]
fn soft_limit(x: f32) -> f32 {
    const THRESHOLD: f32 = 28000.0;
    const MAX: f32 = 32767.0;
    if x.abs() < THRESHOLD {
        x
    } else {
        let sign = x.signum();
        let over = (x.abs() - THRESHOLD) / (MAX - THRESHOLD);
        sign * (THRESHOLD + (MAX - THRESHOLD) * over.tanh())
    }
}

/// Smooth entry/exit noise gate with hysteresis and RMS tracking.
struct SmarterNoiseGate {
    channels: usize,
    in_buf: Vec<f32>,
    in_head: usize,
    out_buf: Vec<f32>,
    out_head: usize,

    // Per-channel state
    envelope: Vec<f32>,
    gain: Vec<f32>,
    hold_count: Vec<usize>,
    is_open: Vec<bool>,

    // Pre-calculated coefficients
    env_alpha_up: f32,
    env_alpha_down: f32,
    attack_alpha: f32,
    release_alpha: f32,
    hold_frames: usize,
    threshold_linear: f32,
    hysteresis_linear: f32,
    min_gain_linear: f32,

    dc_x: Vec<f32>,
    dc_y: Vec<f32>,
    frame_in: Vec<Box<[f32; AUDIO_FRAME_SIZE]>>,
}

impl SmarterNoiseGate {
    const DC_COEFF: f32 = 0.9975;
    const SAMPLE_RATE: f32 = 48000.0;

    fn new(channels: usize, mut settings: NoiseSuppressorSettings) -> Self {
        settings.sanitize();

        // 3ms envelope attack, 15ms release for a more stable detector
        let env_alpha_up = 1.0 - (-1.0 / (Self::SAMPLE_RATE * 0.003)).exp();
        let env_alpha_down = 1.0 - (-1.0 / (Self::SAMPLE_RATE * 0.015)).exp();

        let attack_alpha = 1.0 - (-1.0 / (Self::SAMPLE_RATE * (settings.attack_ms / 1000.0))).exp();
        let release_alpha =
            1.0 - (-1.0 / (Self::SAMPLE_RATE * (settings.release_ms / 1000.0))).exp();
        let hold_frames = ((settings.hold_ms / 1000.0) * Self::SAMPLE_RATE) as usize;

        let threshold_linear = 10f32.powf(settings.gate_threshold_db / 20.0);
        let hysteresis_linear =
            10f32.powf((settings.gate_threshold_db + settings.gate_hysteresis_db) / 20.0);
        let min_gain_linear = 10f32.powf(settings.min_gain_db / 20.0);

        Self {
            channels,
            in_buf: Vec::new(),
            in_head: 0,
            out_buf: Vec::new(),
            out_head: 0,
            envelope: vec![0.0; channels],
            gain: vec![0.0; channels],
            hold_count: vec![0; channels],
            is_open: vec![false; channels],
            env_alpha_up,
            env_alpha_down,
            attack_alpha,
            release_alpha,
            hold_frames,
            threshold_linear,
            hysteresis_linear,
            min_gain_linear,
            dc_x: vec![0.0; channels],
            dc_y: vec![0.0; channels],
            frame_in: (0..channels)
                .map(|_| Box::new([0.0; AUDIO_FRAME_SIZE]))
                .collect(),
        }
    }

    #[inline]
    fn dc_block(&mut self, ch: usize, x: f32) -> f32 {
        let y = x - self.dc_x[ch] + Self::DC_COEFF * self.dc_y[ch];
        self.dc_x[ch] = x;
        self.dc_y[ch] = y;
        y
    }

    #[allow(clippy::needless_range_loop)]
    fn process(&mut self, data: &mut [u8]) {
        if data.len() % (self.channels * 2) != 0 {
            return;
        }

        let samples = unsafe {
            std::slice::from_raw_parts_mut(data.as_mut_ptr() as *mut i16, data.len() / 2)
        };

        for s in samples.iter() {
            self.in_buf.push(*s as f32);
        }

        let interleaved_frame = AUDIO_FRAME_SIZE * self.channels;
        while (self.in_buf.len() - self.in_head) >= interleaved_frame {
            let out_start = self.out_buf.len();
            self.out_buf.resize(out_start + interleaved_frame, 0.0);
            let frame_start = self.in_head;

            for ch in 0..self.channels {
                for i in 0..AUDIO_FRAME_SIZE {
                    self.frame_in[ch][i] = self.in_buf[frame_start + i * self.channels + ch];
                }

                let mut current_env = self.envelope[ch];
                let mut current_gain = self.gain[ch];
                let mut current_hold = self.hold_count[ch];
                let mut gate_open = self.is_open[ch];

                for i in 0..AUDIO_FRAME_SIZE {
                    let s_abs = self.frame_in[ch][i].abs();

                    // Smoother envelope tracking (asymmetric)
                    let env_alpha = if s_abs > current_env {
                        self.env_alpha_up
                    } else {
                        self.env_alpha_down
                    };
                    current_env += env_alpha * (s_abs - current_env);

                    // Gate logic with hysteresis
                    if gate_open {
                        if current_env < self.threshold_linear {
                            if current_hold > 0 {
                                current_hold -= 1;
                            } else {
                                gate_open = false;
                            }
                        } else {
                            current_hold = self.hold_frames;
                        }
                    } else {
                        if current_env > self.hysteresis_linear {
                            gate_open = true;
                            current_hold = self.hold_frames;
                        }
                    }

                    let target_gain = if gate_open { 1.0 } else { self.min_gain_linear };

                    // Gain smoothing
                    if target_gain > current_gain {
                        current_gain += self.attack_alpha * (target_gain - current_gain);
                    } else {
                        current_gain += self.release_alpha * (target_gain - current_gain);
                    }

                    let processed = self.dc_block(ch, self.frame_in[ch][i] * current_gain);
                    self.out_buf[out_start + i * self.channels + ch] = soft_limit(processed);
                }

                self.envelope[ch] = current_env;
                self.gain[ch] = current_gain;
                self.hold_count[ch] = current_hold;
                self.is_open[ch] = gate_open;
            }

            self.in_head += interleaved_frame;
            if self.in_head > 0 && self.in_head >= self.in_buf.len() / 2 {
                self.in_buf.drain(0..self.in_head);
                self.in_head = 0;
            }
        }

        let available = (self.out_buf.len() - self.out_head).min(samples.len());
        for i in 0..available {
            samples[i] = self.out_buf[self.out_head + i].clamp(-32768.0, 32767.0) as i16;
        }

        self.out_head += available;
        if self.out_head > 0 && self.out_head >= self.out_buf.len() / 2 {
            self.out_buf.drain(0..self.out_head);
            self.out_head = 0;
        }

        for i in available..samples.len() {
            samples[i] = 0;
        }
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
}
