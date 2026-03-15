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
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
    AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, WAVEFORMATEX,
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

        let mut noise_processor = if config.bits_per_sample == 16 && config.noise_reduction_enabled
        {
            tracing::warn!(
                "RNNoise: INITIALIZING noise processor for {} channels",
                config.channels
            );
            Some(MicNoiseProcessor::RNNoise(RNNoiseProcessor::new(
                config.channels as usize,
            )))
        } else {
            tracing::warn!(
                "RNNoise: Processor NOT initialized (bits: {}, enabled: {})",
                config.bits_per_sample,
                config.noise_reduction_enabled
            );
            None
        };

        let mut last_activity_log = std::time::Instant::now();
        let mut packets_since_log = 0;
        let mut total_frames: u64 = 0;

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
                    packets_since_log += 1;

                    if last_activity_log.elapsed() >= Duration::from_secs(2) {
                        tracing::info!(
                            "RNNoise: Active - processed {} packets in last 2s",
                            packets_since_log
                        );
                        packets_since_log = 0;
                        last_activity_log = std::time::Instant::now();
                    }
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

enum MicNoiseProcessor {
    RNNoise(RNNoiseProcessor),
}

impl MicNoiseProcessor {
    fn process(&mut self, data: &mut [u8]) {
        match self {
            Self::RNNoise(processor) => processor.process(data),
        }
    }
}

#[inline]
fn soft_limit(x: f32) -> f32 {
    const THRESHOLD: f32 = 28_000.0;
    const MAX: f32 = 32_767.0;

    if x.abs() < THRESHOLD {
        x
    } else {
        let sign = x.signum();
        let over = (x.abs() - THRESHOLD) / (MAX - THRESHOLD);
        sign * (THRESHOLD + (MAX - THRESHOLD) * over.tanh())
    }
}

/// RNNoise-based noise suppression using the nnnoiseless crate.
///
/// Pure RNNoise is fairly subtle on steady mic hiss, so this adapter keeps the
/// earlier VAD/SNR-driven gain shaping on top of the denoiser output to make
/// the suppression clearly audible in the live mic path.
struct RNNoiseProcessor {
    channels: usize,
    states: Vec<Box<DenoiseState<'static>>>,
    in_buf: Vec<f32>,
    in_head: usize,
    out_buf: Vec<f32>,
    out_head: usize,
    gain: Vec<f32>,
    noise_floor: Vec<f32>,
    speech_hangover: Vec<u8>,
    dc_x: Vec<f32>,
    dc_y: Vec<f32>,
    frame_in: Vec<Box<[f32; AUDIO_FRAME_SIZE]>>,
    frame_out: Vec<Box<[f32; AUDIO_FRAME_SIZE]>>,
    primed: bool,
    attack_alpha: f32,
    release_alpha: f32,
}

impl RNNoiseProcessor {
    const DC_COEFF: f32 = 0.9975;
    const MIN_GAIN: f32 = 0.004;
    const VAD_NOISE_THRESHOLD: f32 = 0.25;
    const VAD_GATE_THRESHOLD: f32 = 0.55;
    const SNR_MIN: f32 = 1.2;
    const SNR_MAX: f32 = 6.0;
    const HANGOVER_FRAMES: u8 = 10;
    const NOISE_FLOOR_FAST_ALPHA: f32 = 0.10;
    const NOISE_FLOOR_SLOW_ALPHA: f32 = 0.01;

    fn new(channels: usize) -> Self {
        let mut states = Vec::with_capacity(channels);
        for _ in 0..channels {
            states.push(DenoiseState::new());
        }

        Self {
            channels,
            states,
            in_buf: Vec::with_capacity(AUDIO_FRAME_SIZE * channels * 4),
            in_head: 0,
            out_buf: Vec::with_capacity(AUDIO_FRAME_SIZE * channels * 4),
            out_head: 0,
            gain: vec![0.0; channels],
            noise_floor: vec![300.0; channels],
            speech_hangover: vec![0; channels],
            dc_x: vec![0.0; channels],
            dc_y: vec![0.0; channels],
            frame_in: (0..channels)
                .map(|_| Box::new([0.0; AUDIO_FRAME_SIZE]))
                .collect(),
            frame_out: (0..channels)
                .map(|_| Box::new([0.0; AUDIO_FRAME_SIZE]))
                .collect(),
            primed: false,
            attack_alpha: Self::alpha_from_ms(1.0),
            release_alpha: Self::alpha_from_ms(30.0),
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
    fn dc_block(&mut self, channel: usize, sample: f32) -> f32 {
        let filtered = sample - self.dc_x[channel] + Self::DC_COEFF * self.dc_y[channel];
        self.dc_x[channel] = sample;
        self.dc_y[channel] = filtered;
        filtered
    }

    #[allow(clippy::needless_range_loop)]
    fn process(&mut self, data: &mut [u8]) {
        if self.channels == 0 || data.len() % (self.channels * 2) != 0 {
            return;
        }

        let samples = unsafe {
            std::slice::from_raw_parts_mut(data.as_mut_ptr() as *mut i16, data.len() / 2)
        };

        self.in_buf.reserve(samples.len());
        for sample in samples.iter() {
            self.in_buf.push(*sample as f32);
        }

        let frame_size = AUDIO_FRAME_SIZE * self.channels;
        while (self.in_buf.len() - self.in_head) >= frame_size {
            let out_start = self.out_buf.len();
            self.out_buf.resize(out_start + frame_size, 0.0);
            let frame_start = self.in_head;

            for channel in 0..self.channels {
                for index in 0..AUDIO_FRAME_SIZE {
                    self.frame_in[channel][index] =
                        self.in_buf[frame_start + index * self.channels + channel];
                }

                let vad = self.states[channel].process_frame(
                    self.frame_out[channel].as_mut_slice(),
                    self.frame_in[channel].as_slice(),
                );

                let mut energy = 0.0;
                for index in 0..AUDIO_FRAME_SIZE {
                    let sample = self.frame_out[channel][index];
                    energy += sample * sample;
                }
                let frame_rms = (energy / AUDIO_FRAME_SIZE as f32).sqrt();

                let floor = &mut self.noise_floor[channel];
                let floor_alpha = if vad < Self::VAD_NOISE_THRESHOLD {
                    Self::NOISE_FLOOR_FAST_ALPHA
                } else {
                    Self::NOISE_FLOOR_SLOW_ALPHA
                };
                *floor += floor_alpha * (frame_rms - *floor);
                *floor = floor.max(10.0);

                let snr = frame_rms / (*floor + 1.0);
                let snr_gate =
                    ((snr - Self::SNR_MIN) / (Self::SNR_MAX - Self::SNR_MIN)).clamp(0.0, 1.0);
                let vad_gate = ((vad - Self::VAD_NOISE_THRESHOLD)
                    / (Self::VAD_GATE_THRESHOLD - Self::VAD_NOISE_THRESHOLD))
                    .clamp(0.0, 1.0);

                let mut gate_confidence = vad_gate.max(snr_gate);
                if vad >= Self::VAD_GATE_THRESHOLD {
                    self.speech_hangover[channel] = Self::HANGOVER_FRAMES;
                } else if self.speech_hangover[channel] > 0 {
                    self.speech_hangover[channel] -= 1;
                    gate_confidence = gate_confidence.max(0.60);
                }

                let target_gain = Self::MIN_GAIN + (1.0 - Self::MIN_GAIN) * gate_confidence;
                let mut current_gain = self.gain[channel];
                let discard_frame = !self.primed;

                for index in 0..AUDIO_FRAME_SIZE {
                    let gain_alpha = if target_gain > current_gain {
                        self.attack_alpha
                    } else {
                        self.release_alpha
                    };
                    current_gain += gain_alpha * (target_gain - current_gain);

                    let denoised = if discard_frame {
                        0.0
                    } else {
                        self.frame_out[channel][index] * current_gain
                    };
                    let dc_blocked = self.dc_block(channel, denoised);
                    self.out_buf[out_start + index * self.channels + channel] =
                        soft_limit(dc_blocked);
                }

                self.gain[channel] = current_gain;
            }

            self.primed = true;
            self.in_head += frame_size;
            if self.in_head > 0 && self.in_head >= self.in_buf.len() / 2 {
                self.in_buf.drain(0..self.in_head);
                self.in_head = 0;
            }
        }

        let available = (self.out_buf.len() - self.out_head).min(samples.len());
        for index in 0..available {
            samples[index] = self.out_buf[self.out_head + index].clamp(-32768.0, 32767.0) as i16;
        }

        self.out_head += available;
        if self.out_head > 0 && self.out_head >= self.out_buf.len() / 2 {
            self.out_buf.drain(0..self.out_head);
            self.out_head = 0;
        }

        for sample in &mut samples[available..] {
            *sample = 0;
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

    #[test]
    fn test_rnnoise_processor_latency() {
        let mut processor = RNNoiseProcessor::new(1);

        // Input 480 samples (10ms at 48kHz) of strong signal
        let mut data: Vec<u8> = (0..480)
            .map(|i| ((i as f32 * 0.1).sin() * 20000.0) as i16)
            .flat_map(|s| s.to_ne_bytes())
            .collect();
        processor.process(&mut data);

        // Should NOT be all zeros because we provided exactly one frame
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
}
