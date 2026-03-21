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
                    tracing::debug!("RNNoise: processed {} packets in 2s", packets);
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
    state: Box<DenoiseState<'static>>,
    state2: Box<DenoiseState<'static>>,
    in_buf: Vec<f32>,
    in_head: usize,
    out_buf: Vec<f32>,
    out_head: usize,
    pending_wet_drop: usize,
    discard_warmup_frame: bool,
    gate_gain: f32,
    dc_x: Vec<f32>,
    dc_y: Vec<f32>,
    /// High-pass filter state: previous input and output per channel (mono only needs 1)
    hp_x_prev: f32,
    hp_y_prev: f32,
    packet_mono: Vec<f32>,
    frame_in: Box<[f32; AUDIO_FRAME_SIZE]>,
    mid_frame: Box<[f32; AUDIO_FRAME_SIZE]>,
    frame_out: Box<[f32; AUDIO_FRAME_SIZE]>,
}

impl RNNoiseProcessor {
    const DC_COEFF: f32 = 0.9975;
    const GATE_OPEN_THRESHOLD: f32 = 0.60;
    const GATE_ATTACK: f32 = 0.90;
    const GATE_RELEASE: f32 = 0.035;
    const GATE_FLOOR: f32 = 0.001;
    /// High-pass filter coefficient for ~80 Hz at 48 kHz.
    /// Computed as: RC / (RC + dt) where RC = 1/(2*pi*f), dt = 1/48000
    /// Removes low-frequency hum and hiss before RNNoise processing.
    const HP_COEFF: f32 = 0.9895;

    fn new(channels: usize) -> Self {
        let mut in_buf = Vec::with_capacity(AUDIO_FRAME_SIZE * 16);
        in_buf.resize(AUDIO_FRAME_SIZE, 0.0);
        let out_buf = Vec::with_capacity(AUDIO_FRAME_SIZE * 16);
        Self {
            channels,
            state: DenoiseState::new(),
            state2: DenoiseState::new(),
            in_buf,
            in_head: 0,
            out_buf,
            out_head: 0,
            pending_wet_drop: 0,
            discard_warmup_frame: true,
            gate_gain: Self::GATE_FLOOR,
            dc_x: vec![0.0; channels],
            dc_y: vec![0.0; channels],
            hp_x_prev: 0.0,
            hp_y_prev: 0.0,
            packet_mono: Vec::with_capacity(AUDIO_FRAME_SIZE * 2),
            frame_in: Box::new([0.0; AUDIO_FRAME_SIZE]),
            mid_frame: Box::new([0.0; AUDIO_FRAME_SIZE]),
            frame_out: Box::new([0.0; AUDIO_FRAME_SIZE]),
        }
    }

    #[inline]
    fn dc_block(&mut self, channel: usize, sample: f32) -> f32 {
        let filtered = sample - self.dc_x[channel] + Self::DC_COEFF * self.dc_y[channel];
        self.dc_x[channel] = sample;
        self.dc_y[channel] = filtered;
        filtered
    }

    /// Single-pole high-pass filter applied to the mono mix before RNNoise.
    /// Removes sub-80 Hz hum and hiss without touching speech frequencies.
    #[inline]
    fn hp_filter(&mut self, sample: f32) -> f32 {
        let y = Self::HP_COEFF * (self.hp_y_prev + sample - self.hp_x_prev);
        self.hp_x_prev = sample;
        self.hp_y_prev = y;
        y
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
    fn skip_pending_wet_output(&mut self) {
        if self.pending_wet_drop == 0 {
            return;
        }

        let available = self.out_buf.len().saturating_sub(self.out_head);
        let skipped = available.min(self.pending_wet_drop);
        self.out_head += skipped;
        self.pending_wet_drop -= skipped;

        if skipped > 0 {
            Self::compact_queue(&mut self.out_buf, &mut self.out_head);
        }
    }

    #[inline]
    fn clamp_i16(sample: f32) -> i16 {
        sample.clamp(-32768.0, 32767.0) as i16
    }

    fn prepare_packet_mono(&mut self, samples: &[i16], frame_count: usize) {
        self.packet_mono.resize(frame_count, 0.0);
        match self.channels {
            1 => {
                for (dst, &sample) in self.packet_mono.iter_mut().zip(samples.iter().take(frame_count)) {
                    *dst = sample as f32;
                }
            }
            2 => {
                for i in 0..frame_count {
                    let idx = i * 2;
                    self.packet_mono[i] = (samples[idx] as f32 + samples[idx + 1] as f32) * 0.5;
                }
            }
            _ => {
                let channels_f = self.channels as f32;
                for i in 0..frame_count {
                    let base = i * self.channels;
                    let mut sum = 0.0f32;
                    for c in 0..self.channels {
                        sum += samples[base + c] as f32;
                    }
                    self.packet_mono[i] = sum / channels_f;
                }
            }
        }
        // Apply high-pass filter inline to avoid borrow conflicts
        for sample in self.packet_mono.iter_mut() {
            let y = Self::HP_COEFF * (self.hp_y_prev + *sample - self.hp_x_prev);
            self.hp_x_prev = *sample;
            self.hp_y_prev = y;
            *sample = y;
        }
    }

    fn write_sample_with_delta(
        &mut self,
        samples: &mut [i16],
        frame_index: usize,
        delta: f32,
        apply_delta: bool,
    ) {
        match self.channels {
            1 => {
                let sample = if apply_delta {
                    samples[frame_index] as f32 + delta
                } else {
                    samples[frame_index] as f32
                };
                samples[frame_index] = Self::clamp_i16(self.dc_block(0, sample));
            }
            2 => {
                let base = frame_index * 2;
                for channel in 0..2 {
                    let sample = if apply_delta {
                        samples[base + channel] as f32 + delta
                    } else {
                        samples[base + channel] as f32
                    };
                    samples[base + channel] = Self::clamp_i16(self.dc_block(channel, sample));
                }
            }
            _ => {
                let base = frame_index * self.channels;
                for channel in 0..self.channels {
                    let sample = if apply_delta {
                        samples[base + channel] as f32 + delta
                    } else {
                        samples[base + channel] as f32
                    };
                    samples[base + channel] = Self::clamp_i16(self.dc_block(channel, sample));
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

        self.prepare_packet_mono(samples, frame_count);

        self.in_buf.extend_from_slice(&self.packet_mono);

        while (self.in_buf.len() - self.in_head) >= AUDIO_FRAME_SIZE {
            let frame_start_idx = self.in_head;
            self.frame_in
                .copy_from_slice(&self.in_buf[frame_start_idx..frame_start_idx + AUDIO_FRAME_SIZE]);

            self.state
                .process_frame(self.mid_frame.as_mut_slice(), self.frame_in.as_slice());
            let speech_prob = self
                .state2
                .process_frame(self.frame_out.as_mut_slice(), self.mid_frame.as_slice());

            let target_gain = if speech_prob > Self::GATE_OPEN_THRESHOLD {
                1.0_f32
            } else {
                Self::GATE_FLOOR
            };
            let coeff = if target_gain > self.gate_gain {
                Self::GATE_ATTACK
            } else {
                Self::GATE_RELEASE
            };
            self.gate_gain += (target_gain - self.gate_gain) * coeff;

            if self.discard_warmup_frame {
                self.discard_warmup_frame = false;
            } else {
                if self.gate_gain < 0.999 {
                    let g = self.gate_gain;
                    for s in self.frame_out.iter_mut() {
                        *s *= g;
                    }
                }
                self.out_buf.extend_from_slice(self.frame_out.as_slice());
            }

            self.in_head += AUDIO_FRAME_SIZE;
            Self::compact_queue(&mut self.in_buf, &mut self.in_head);
        }

        self.skip_pending_wet_output();
        let available = (self.out_buf.len() - self.out_head).min(frame_count);

        for i in 0..available {
            let wet_mono = self.out_buf[self.out_head + i];
            let dry_mono = self.packet_mono[i];
            self.write_sample_with_delta(samples, i, wet_mono - dry_mono, true);
        }

        if available > 0 {
            self.out_head += available;
            Self::compact_queue(&mut self.out_buf, &mut self.out_head);
        }

        for i in available..frame_count {
            self.write_sample_with_delta(samples, i, 0.0, false);
        }

        self.pending_wet_drop = self.pending_wet_drop.saturating_add(frame_count - available);
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
    fn test_rnnoise_stereo_preserves_channel_difference() {
        let mut processor = RNNoiseProcessor::new(2);

        let mut data: Vec<u8> = Vec::with_capacity(960 * 2 * 2);
        for _ in 0..960 {
            data.extend_from_slice(&1000i16.to_ne_bytes());
            data.extend_from_slice(&3000i16.to_ne_bytes());
        }

        processor.process(&mut data);

        let samples: &[i16] =
            unsafe { std::slice::from_raw_parts(data.as_ptr() as *const i16, data.len() / 2) };
        assert!(samples[960..]
            .chunks_exact(2)
            .any(|pair| pair[0] != pair[1]));
    }
}
