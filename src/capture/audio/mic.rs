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
    pub min_gain: f32,
    pub vad_noise_threshold: f32,
    pub vad_gate_threshold: f32,
    pub snr_min: f32,
    pub snr_max: f32,
    pub hangover_frames: u8,
    pub noise_floor_fast_alpha: f32,
    pub noise_floor_slow_alpha: f32,
    pub attack_alpha: f32,
    pub release_alpha: f32,
}

impl Default for NoiseSuppressorSettings {
    fn default() -> Self {
        Self {
            min_gain: 0.004,
            vad_noise_threshold: 0.25,
            vad_gate_threshold: 0.55,
            snr_min: 1.2,
            snr_max: 6.0,
            hangover_frames: 10,
            noise_floor_fast_alpha: 0.10,
            noise_floor_slow_alpha: 0.01,
            attack_alpha: Self::alpha_from_ms(1.0),
            release_alpha: Self::alpha_from_ms(30.0),
        }
    }
}

impl NoiseSuppressorSettings {
    #[inline]
    pub fn alpha_from_ms(ms: f32) -> f32 {
        if ms <= 0.0 {
            return 1.0;
        }
        let sample_rate = 48_000.0;
        let tau_seconds = ms / 1000.0;
        let alpha = 1.0 - (-1.0 / (sample_rate * tau_seconds)).exp();
        alpha.clamp(0.000001, 1.0)
    }

    fn sanitize(&mut self) {
        self.min_gain = self.min_gain.clamp(0.0, 0.2);
        self.vad_noise_threshold = self.vad_noise_threshold.clamp(0.0, 0.95);
        self.vad_gate_threshold = self.vad_gate_threshold.clamp(0.05, 1.0);
        if self.vad_gate_threshold <= self.vad_noise_threshold {
            self.vad_gate_threshold = (self.vad_noise_threshold + 0.01).min(1.0);
        }

        self.snr_min = self.snr_min.clamp(0.5, 10.0);
        self.snr_max = self.snr_max.clamp(1.0, 15.0);
        if self.snr_max <= self.snr_min {
            self.snr_max = (self.snr_min + 0.1).min(15.0);
        }

        self.noise_floor_fast_alpha = self.noise_floor_fast_alpha.clamp(0.001, 0.5);
        self.noise_floor_slow_alpha = self.noise_floor_slow_alpha.clamp(0.001, 0.2);
        if self.noise_floor_slow_alpha > self.noise_floor_fast_alpha {
            self.noise_floor_slow_alpha = self.noise_floor_fast_alpha;
        }

        self.attack_alpha = self.attack_alpha.clamp(0.000001, 1.0);
        self.release_alpha = self.release_alpha.clamp(0.000001, 1.0);
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
    pub noise_reduction: bool,     // Enable AI noise reduction (nnnoiseless)
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
            noise_reduction: true,
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

        let mut noise_processor = if config.noise_reduction
            && config.sample_rate == 48000
            && config.bits_per_sample == 16
        {
            Some(NoiseSuppressor::new(
                config.channels as usize,
                config.noise_suppressor_settings,
            ))
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

/// Advanced noise suppressor using `nnnoiseless` (RNNoise) for 48kHz 16-bit PCM audio.
///
/// Key improvements:
///
/// 1. **VAD-based adaptive gain** — uses the voice-activity probability returned by RNNoise
///    to smoothly gate between denoised and attenuated audio instead of slamming to silence.
/// 2. **Adaptive noise-floor tracking** — estimates each channel's background floor and
///    closes the gate harder when SNR is poor.
/// 3. **Per-sample smooth gain interpolation** — eliminates discontinuities at frame boundaries.
/// 4. **Speech hangover** — keeps speech tails natural while still cutting static between words.
/// 5. **DC blocking filter** — removes low-frequency drift that can build up across frames.
/// 6. **Soft limiter** — prevents clipping without hard edges.
struct NoiseSuppressor {
    channels: usize,
    states: Vec<Box<nnnoiseless::DenoiseState<'static>>>,
    settings: NoiseSuppressorSettings,

    /// Interleaved f32 input accumulator.
    in_buf: Vec<f32>,

    /// Interleaved f32 output accumulator.
    out_buf: Vec<f32>,

    /// Per-channel smoothed gain from the *previous* frame.
    gain: Vec<f32>,

    /// Per-channel running background level estimate (RMS in i16 domain).
    noise_floor: Vec<f32>,

    /// Per-channel gate hangover (in frames) to avoid chopping trailing syllables.
    speech_hangover: Vec<u8>,

    /// Per-channel DC-blocker state: last input, last output.
    dc_x: Vec<f32>,
    dc_y: Vec<f32>,
}

impl NoiseSuppressor {
    /// RNNoise fixed frame size: 480 samples (10 ms at 48 kHz).
    const FRAME_SIZE: usize = 480;

    /// DC blocking filter coefficient (HPF at ~20 Hz for 48 kHz).
    const DC_COEFF: f32 = 0.9975;

    #[allow(clippy::needless_range_loop)]
    fn new(channels: usize, mut settings: NoiseSuppressorSettings) -> Self {
        let mut states = Vec::with_capacity(channels);
        for _ in 0..channels {
            states.push(nnnoiseless::DenoiseState::new());
        }

        settings.sanitize();

        Self {
            channels,
            in_buf: Vec::new(),
            out_buf: Vec::new(),
            settings,
            gain: vec![0.0; channels],
            noise_floor: vec![300.0; channels],
            speech_hangover: vec![0; channels],
            dc_x: vec![0.0; channels],
            dc_y: vec![0.0; channels],
            states,
        }
    }

    /// Apply DC-blocking high-pass filter to a single sample for the given channel.
    #[inline]
    fn dc_block(&mut self, ch: usize, x: f32) -> f32 {
        // y[n] = x[n] - x[n-1] + R * y[n-1],  R ≈ 0.9975
        let y = x - self.dc_x[ch] + Self::DC_COEFF * self.dc_y[ch];
        self.dc_x[ch] = x;
        self.dc_y[ch] = y;
        y
    }

    /// Soft-limit a sample to [-32767, 32767] using tanh-style saturation.
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

    #[allow(clippy::needless_range_loop)]
    fn process(&mut self, data: &mut [u8]) {
        if data.len() % (self.channels * 2) != 0 {
            return;
        }

        let samples = unsafe {
            std::slice::from_raw_parts_mut(data.as_mut_ptr() as *mut i16, data.len() / 2)
        };

        // Append incoming i16 → f32 to the input accumulator
        self.in_buf.reserve(samples.len());
        for s in samples.iter() {
            self.in_buf.push(*s as f32);
        }

        let interleaved_frame = Self::FRAME_SIZE * self.channels;

        while self.in_buf.len() >= interleaved_frame {
            let out_start = self.out_buf.len();
            self.out_buf.resize(out_start + interleaved_frame, 0.0);

            for ch in 0..self.channels {
                // De-interleave this channel's FRAME_SIZE samples
                let mut channel_in = [0.0f32; Self::FRAME_SIZE];
                for i in 0..Self::FRAME_SIZE {
                    channel_in[i] = self.in_buf[i * self.channels + ch];
                }

                // Run RNNoise — returns VAD probability [0.0, 1.0]
                let mut channel_denoised = [0.0f32; Self::FRAME_SIZE];
                let vad = self.states[ch].process_frame(&mut channel_denoised, &channel_in);

                // Measure per-frame RMS in the i16 amplitude domain.
                let mut energy = 0.0f32;
                for sample in channel_denoised {
                    energy += sample * sample;
                }
                let frame_rms = (energy / Self::FRAME_SIZE as f32).sqrt();

                // Update background floor. Learn quickly in non-speech, slowly otherwise.
                let floor = &mut self.noise_floor[ch];
                let alpha = if vad < self.settings.vad_noise_threshold {
                    self.settings.noise_floor_fast_alpha
                } else {
                    self.settings.noise_floor_slow_alpha
                };
                *floor += alpha * (frame_rms - *floor);
                *floor = floor.max(10.0);

                // SNR-derived gate confidence. Low SNR means likely static/noise.
                let snr = frame_rms / (*floor + 1.0);
                let snr_gate = ((snr - self.settings.snr_min)
                    / (self.settings.snr_max - self.settings.snr_min))
                    .clamp(0.0, 1.0);

                // VAD-derived confidence.
                let vad_gate = ((vad - self.settings.vad_noise_threshold)
                    / (self.settings.vad_gate_threshold - self.settings.vad_noise_threshold))
                    .clamp(0.0, 1.0);

                // --- Adaptive gain from VAD ---
                let mut gate_confidence = vad_gate.max(snr_gate);

                if vad >= self.settings.vad_gate_threshold {
                    self.speech_hangover[ch] = self.settings.hangover_frames;
                } else if self.speech_hangover[ch] > 0 {
                    self.speech_hangover[ch] -= 1;
                    // Keep some openness for trailing phonemes while release smoothing handles fade-out.
                    gate_confidence = gate_confidence.max(0.60);
                }

                let target_gain =
                    self.settings.min_gain + (1.0 - self.settings.min_gain) * gate_confidence;

                let mut current_gain = self.gain[ch];

                for i in 0..Self::FRAME_SIZE {
                    // Smooth gain toward target_gain per sample
                    let alpha = if target_gain > current_gain {
                        self.settings.attack_alpha
                    } else {
                        self.settings.release_alpha
                    };
                    current_gain += alpha * (target_gain - current_gain);

                    let denoised = channel_denoised[i] * current_gain;

                    let dc_blocked = self.dc_block(ch, denoised);
                    let limited = Self::soft_limit(dc_blocked);

                    self.out_buf[out_start + i * self.channels + ch] = limited;
                }

                self.gain[ch] = current_gain;
            }

            // Advance the input buffer by one full frame (RNNoise works in contiguous blocks)
            self.in_buf.drain(0..interleaved_frame);
        }

        // --- Write processed output back to the i16 buffer ---
        let available = self.out_buf.len().min(samples.len());

        for i in 0..available {
            samples[i] = self.out_buf[i].clamp(-32768.0, 32767.0) as i16;
        }
        self.out_buf.drain(0..available);

        // Zero-fill if we don't have enough output yet (startup latency)
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
        assert!(config.noise_reduction);
    }
}
