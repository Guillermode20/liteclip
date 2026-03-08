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

/// Configuration for WASAPI microphone capture
#[derive(Debug, Clone)]
pub struct WasapiMicConfig {
    pub sample_rate: u32,
    pub channels: u16,
    pub bits_per_sample: u16,
    pub buffer_duration: Duration,
    pub device_id: Option<String>, // None for default device
    pub noise_reduction: bool,     // Enable AI noise reduction (nnnoiseless)
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

    /// Check if the capture loop has successfully initialized WASAPI
    pub fn is_initialized(&self) -> bool {
        self.initialized.load(Ordering::SeqCst)
    }

    /// Get the number of samples processed
    pub fn samples_processed(&self) -> u64 {
        self.processed_samples.load(Ordering::SeqCst)
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
            Some(NoiseSuppressor::new(config.channels as usize))
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
/// Key improvements over a naive frame-by-frame approach:
///
/// 1. **Overlap-add with Hann window (50% hop)** — eliminates discontinuities at frame
///    boundaries that cause crackling.
/// 2. **VAD-based adaptive gain** — uses the voice-activity probability returned by RNNoise
///    to smoothly gate between denoised and attenuated audio instead of slamming to silence.
/// 3. **Attack / release smoothing** — fast attack (≈2 ms) to catch speech onsets, slow
///    release (≈80 ms) to let natural tails ring out.
/// 4. **Comfort noise** — injects a very quiet shaped noise floor so silence never sounds
///    unnaturally dead.
/// 5. **DC blocking filter** — removes low-frequency drift that can build up across frames.
/// 6. **Soft limiter** — prevents clipping without hard edges.
struct NoiseSuppressor {
    channels: usize,
    states: Vec<Box<nnnoiseless::DenoiseState<'static>>>,

    /// Per-channel ring of the *previous* windowed frame (for overlap-add output).
    prev_frame: Vec<[f32; Self::FRAME_SIZE]>,

    /// Interleaved f32 input accumulator.
    in_buf: Vec<f32>,

    /// Interleaved f32 output accumulator (ready to convert back to i16).
    out_buf: Vec<f32>,

    /// Per-channel smoothed gain (0.0 = silent, 1.0 = full signal).
    gain: Vec<f32>,

    /// Per-channel DC-blocker state: last input, last output.
    dc_x: Vec<f32>,
    dc_y: Vec<f32>,

    /// Pre-computed Hann analysis window (length = FRAME_SIZE).
    window: [f32; Self::FRAME_SIZE],

    /// Simple PRNG state for comfort noise.
    rng_state: u32,

    /// Whether this is the very first frame (skip overlap for first).
    first_frame: bool,
}

impl NoiseSuppressor {
    /// RNNoise fixed frame size: 480 samples (10 ms at 48 kHz).
    const FRAME_SIZE: usize = 480;
    /// Hop size for 50% overlap.
    const HOP_SIZE: usize = Self::FRAME_SIZE / 2; // 240 samples, 5 ms

    // --- Gain smoothing time constants (in frames, where 1 frame = 5 ms at hop rate) ---
    /// Attack: ~2 ms → coefficient per hop ≈ 0.35
    const ATTACK_COEFF: f32 = 0.35;
    /// Release: ~80 ms → coefficient per hop ≈ 0.04
    const RELEASE_COEFF: f32 = 0.04;

    /// Minimum gain floor — never gate below this. Keeps a whisper of the processed
    /// signal audible so the listener doesn't perceive "pumping".
    const MIN_GAIN: f32 = 0.02;

    /// Comfort noise amplitude (RMS). Very quiet — about −66 dBFS.
    const COMFORT_NOISE_AMP: f32 = 50.0;

    /// VAD threshold below which we consider the frame "non-speech" and start
    /// closing the gate.
    const VAD_GATE_THRESHOLD: f32 = 0.35;

    /// DC blocking filter coefficient (HPF at ~20 Hz for 48 kHz).
    const DC_COEFF: f32 = 0.9975;

    #[allow(clippy::needless_range_loop)]
    fn new(channels: usize) -> Self {
        let mut states = Vec::with_capacity(channels);
        for _ in 0..channels {
            states.push(nnnoiseless::DenoiseState::new());
        }

        // Pre-compute Hann window
        let mut window = [0.0f32; Self::FRAME_SIZE];
        for i in 0..Self::FRAME_SIZE {
            let t = i as f32 / (Self::FRAME_SIZE - 1) as f32;
            window[i] = 0.5 * (1.0 - (2.0 * std::f32::consts::PI * t).cos());
        }

        Self {
            channels,
            prev_frame: vec![[0.0; Self::FRAME_SIZE]; channels],
            in_buf: Vec::new(),
            out_buf: Vec::new(),
            gain: vec![0.0; channels],
            dc_x: vec![0.0; channels],
            dc_y: vec![0.0; channels],
            window,
            states,
            rng_state: 0xDEAD_BEEFu32,
            first_frame: true,
        }
    }

    /// Fast xorshift32 PRNG — returns a value in [-1, 1].
    #[inline]
    fn next_noise(&mut self) -> f32 {
        let mut s = self.rng_state;
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        self.rng_state = s;
        // Map u32 → [-1.0, 1.0]
        (s as i32) as f32 / (i32::MAX as f32)
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
        self.in_buf.extend(samples.iter().map(|&s| s as f32));

        // --- Overlap-add processing with 50% hop ---
        //
        // We need at least one full FRAME_SIZE of interleaved samples to produce output.
        // Each iteration we advance by HOP_SIZE interleaved samples, producing HOP_SIZE
        // output samples per channel.
        let interleaved_frame = Self::FRAME_SIZE * self.channels;
        let interleaved_hop = Self::HOP_SIZE * self.channels;

        while self.in_buf.len() >= interleaved_frame {
            // Allocate per-channel scratch on the stack
            let mut channel_in = [0.0f32; Self::FRAME_SIZE];
            let mut channel_denoised = [0.0f32; Self::FRAME_SIZE];

            // We will produce `HOP_SIZE` interleaved output samples
            let out_start = self.out_buf.len();
            self.out_buf.resize(out_start + interleaved_hop, 0.0);

            for ch in 0..self.channels {
                // De-interleave this channel's FRAME_SIZE samples
                for i in 0..Self::FRAME_SIZE {
                    channel_in[i] = self.in_buf[i * self.channels + ch];
                }

                // Run RNNoise — returns VAD probability [0.0, 1.0]
                let vad = self.states[ch].process_frame(&mut channel_denoised, &channel_in);

                // --- Adaptive gain from VAD ---
                let target_gain = if vad >= Self::VAD_GATE_THRESHOLD {
                    // Speech detected: open gate fully
                    1.0
                } else {
                    // Below threshold: map linearly to [MIN_GAIN, partial]
                    // so it doesn't slam shut
                    Self::MIN_GAIN + (1.0 - Self::MIN_GAIN) * (vad / Self::VAD_GATE_THRESHOLD)
                };

                // Smooth gain with asymmetric attack/release
                let coeff = if target_gain > self.gain[ch] {
                    Self::ATTACK_COEFF
                } else {
                    Self::RELEASE_COEFF
                };
                self.gain[ch] += coeff * (target_gain - self.gain[ch]);

                // --- Window, apply gain, overlap-add ---
                // Current windowed frame (full FRAME_SIZE)
                let mut cur_windowed = [0.0f32; Self::FRAME_SIZE];
                for i in 0..Self::FRAME_SIZE {
                    let denoised = channel_denoised[i] * self.gain[ch];
                    // Add comfort noise scaled by (1 - gain) so it only appears in quiet parts
                    let comfort =
                        self.next_noise() * Self::COMFORT_NOISE_AMP * (1.0 - self.gain[ch]);
                    cur_windowed[i] = (denoised + comfort) * self.window[i];
                }

                // Overlap-add: the output for this hop is the *second half* of the previous
                // frame plus the *first half* of the current frame.
                if !self.first_frame {
                    for i in 0..Self::HOP_SIZE {
                        let overlap_sample =
                            self.prev_frame[ch][Self::HOP_SIZE + i] + cur_windowed[i];
                        let dc_blocked = self.dc_block(ch, overlap_sample);
                        let limited = Self::soft_limit(dc_blocked);
                        self.out_buf[out_start + i * self.channels + ch] = limited;
                    }
                } else {
                    // First frame: no previous data, just output the first half directly
                    // with reduced amplitude to fade in
                    for i in 0..Self::HOP_SIZE {
                        let fade = i as f32 / Self::HOP_SIZE as f32; // linear fade-in
                        let sample = cur_windowed[i] * fade;
                        let dc_blocked = self.dc_block(ch, sample);
                        let limited = Self::soft_limit(dc_blocked);
                        self.out_buf[out_start + i * self.channels + ch] = limited;
                    }
                }

                // Store current frame for next overlap
                self.prev_frame[ch] = cur_windowed;
            }

            self.first_frame = false;

            // Advance the input buffer by one hop (not a full frame, because we overlap)
            self.in_buf.drain(0..interleaved_hop);
        }

        // --- Write processed output back to the i16 buffer ---
        let available = self.out_buf.len().min(samples.len());

        for (i, sample) in samples.iter_mut().enumerate().take(available) {
            *sample = self.out_buf[i].clamp(-32768.0, 32767.0) as i16;
        }
        self.out_buf.drain(0..available);

        // Zero-fill if we don't have enough output yet (startup latency)
        for sample in samples.iter_mut().skip(available) {
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
        assert!(config.noise_reduction);
    }
}
