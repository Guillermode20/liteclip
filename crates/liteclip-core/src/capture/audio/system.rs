//! WASAPI System Audio Capture
//!
//! Captures system audio (loopback) using WASAPI in shared mode.

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
    eConsole, eMultimedia, eRender, IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator,
    MMDeviceEnumerator, AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED,
    AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM, AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
    AUDCLNT_STREAMFLAGS_LOOPBACK, AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, WAVEFORMATEX,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED,
};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

use crate::buffer::ring::qpc_frequency;
use crate::encode::{EncodedPacket, StreamType};

/// Configuration for WASAPI system audio capture
#[derive(Debug, Clone)]
pub struct WasapiSystemConfig {
    pub sample_rate: u32,
    pub channels: u16,
    pub bits_per_sample: u16,
    pub buffer_duration: Duration,
    pub device_id: Option<String>, // None for default device
}

impl Default for WasapiSystemConfig {
    fn default() -> Self {
        Self {
            sample_rate: 48000,
            channels: 2,
            bits_per_sample: 16,
            buffer_duration: Duration::from_millis(20),
            device_id: None,
        }
    }
}

/// WASAPI system audio capture implementation
pub struct WasapiSystemCapture {
    running: Arc<AtomicBool>,
    initialized: Arc<AtomicBool>,
    packet_tx: Sender<EncodedPacket>,
    packet_rx: Receiver<EncodedPacket>,
    processed_samples: Arc<AtomicU64>,
    capture_thread: Option<thread::JoinHandle<()>>,
}

impl WasapiSystemCapture {
    /// Create a new WASAPI system audio capture instance
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

    /// Start capturing system audio
    pub fn start(&mut self, config: WasapiSystemConfig) -> Result<()> {
        if self.running.load(Ordering::Relaxed) {
            return Ok(());
        }

        self.running.store(true, Ordering::Relaxed);
        let running = Arc::clone(&self.running);
        let initialized = self.initialized.clone();
        let packet_tx = self.packet_tx.clone();
        let processed_samples = Arc::clone(&self.processed_samples);

        self.capture_thread = Some(thread::spawn(move || {
            if let Err(e) = Self::capture_loop(
                running.clone(),
                initialized,
                packet_tx,
                processed_samples,
                config,
            ) {
                error!("System audio capture error: {}", e);
                running.store(false, Ordering::Relaxed);
            }
        }));

        let mut attempts = 0;
        while !self.initialized.load(Ordering::Relaxed) && self.running.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_millis(50));
            attempts += 1;
            if attempts > 40 {
                return Err(anyhow::anyhow!(
                    "System audio capture initialization timed out"
                ));
            }
        }

        if !self.running.load(Ordering::Relaxed) {
            return Err(anyhow::anyhow!("System audio capture failed to start"));
        }

        debug!("WASAPI system audio capture started");
        Ok(())
    }

    /// Stop capturing system audio
    pub fn stop(&mut self) {
        // Relaxed ordering is sufficient for a simple stop flag - no data synchronization needed
        self.running.store(false, Ordering::Relaxed);
        if let Some(handle) = self.capture_thread.take() {
            if handle.join().is_err() {
                error!("System audio capture thread panicked");
            }
        }
        self.initialized.store(false, Ordering::SeqCst);
        debug!("WASAPI system audio capture stopped");
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
        config: WasapiSystemConfig,
    ) -> Result<()> {
        Self::set_audio_thread_priority();
        debug!("Starting WASAPI system capture loop");

        let _com = ComApartment::initialize()?;

        if config.device_id.is_some() {
            warn!("System audio custom device_id is not implemented yet; using default render endpoint");
        }

        let enumerator: IMMDeviceEnumerator =
            unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
                .context("Failed to create MMDeviceEnumerator")?;

        // Log all available render devices
        crate::capture::audio::device_info::log_all_render_devices(&enumerator);

        let device = unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eMultimedia) }
            .or_else(|_| unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eConsole) })
            .context("Failed to get default render endpoint for loopback capture")?;

        // Log which device was selected for loopback
        crate::capture::audio::device_info::log_device(
            "Selected system audio device (loopback)",
            &device,
        );

        let audio_client: IAudioClient = unsafe { device.Activate(CLSCTX_ALL, None) }
            .context("Failed to activate IAudioClient for system loopback")?;

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
        let stream_flags = AUDCLNT_STREAMFLAGS_LOOPBACK
            | AUDCLNT_STREAMFLAGS_EVENTCALLBACK
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
        .context("Failed to initialize IAudioClient for loopback capture")?;

        let capture_event =
            EventHandle::new().context("Failed to create system audio event handle")?;
        unsafe { audio_client.SetEventHandle(capture_event.raw()) }
            .context("Failed to bind system audio event handle")?;

        let capture_client: IAudioCaptureClient = unsafe { audio_client.GetService() }
            .context("Failed to get IAudioCaptureClient service")?;

        unsafe { audio_client.Start() }.context("Failed to start system audio capture")?;
        let wait_timeout_ms = config.buffer_duration.as_millis().clamp(1, 25) as u32;

        initialized.store(true, Ordering::SeqCst);
        debug!("WASAPI system audio capture loop initialized successfully");

        let start_qpc = query_qpc()?;
        let qpc_freq = qpc_frequency() as f64;
        let sample_rate = config.sample_rate.max(1) as f64;
        let mut total_frames: u64 = 0;
        let mut packet_count: u64 = 0;
        let max_buffer_size = (config.sample_rate as usize / 10) * block_align as usize;
        let mut audio_buffer = BytesMut::with_capacity(max_buffer_size);

        // Relaxed ordering is sufficient - we're just checking a stop flag
        while running.load(Ordering::Relaxed) {
            let mut packet_frames = unsafe { capture_client.GetNextPacketSize() }
                .context("IAudioCaptureClient::GetNextPacketSize failed")?;

            if packet_frames == 0 {
                match unsafe { WaitForSingleObject(capture_event.raw(), wait_timeout_ms) }.0 {
                    0 => {}
                    258 => continue,
                    status => {
                        warn!("System audio wait returned unexpected status: {:?}", status);
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
                    StreamType::SystemAudio,
                );

                if packet_tx.send(packet).is_err() {
                    running.store(false, Ordering::Relaxed);
                    break;
                }

                processed_samples.fetch_add(frame_count as u64, Ordering::SeqCst);
                packet_count = packet_count.saturating_add(1);

                if packet_count == 1 {
                    debug!(
                        "WASAPI system loopback received first packet ({} frames, {} bytes)",
                        frame_count, byte_count
                    );
                } else if packet_count % 250 == 0 {
                    debug!(
                        "WASAPI system loopback packets={}, processed_frames={}",
                        packet_count, total_frames
                    );
                }

                packet_frames = unsafe { capture_client.GetNextPacketSize() }
                    .context("IAudioCaptureClient::GetNextPacketSize failed")?;
            }
        }

        unsafe { audio_client.Stop() }.context("Failed to stop system audio capture")?;

        debug!("WASAPI system audio capture loop ended");
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

struct ComApartment {
    initialized_by_us: bool,
}

impl ComApartment {
    fn initialize() -> Result<Self> {
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if hr.is_err() {
            return Err(anyhow::anyhow!(
                "CoInitializeEx failed for WASAPI system capture: {:?}",
                hr
            ));
        }
        Ok(Self {
            initialized_by_us: hr.is_ok(),
        })
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        if self.initialized_by_us {
            unsafe {
                CoUninitialize();
            }
        }
    }
}

impl Drop for WasapiSystemCapture {
    fn drop(&mut self) {
        self.stop();
    }
}

impl WasapiSystemCapture {
    fn set_audio_thread_priority() {
        #[cfg(windows)]
        {
            use windows::Win32::System::Threading::{
                GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_ABOVE_NORMAL,
            };
            unsafe {
                if let Err(e) = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_ABOVE_NORMAL)
                {
                    warn!("Failed to set system audio thread priority: {}", e);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wasapi_system_config_default() {
        let config = WasapiSystemConfig::default();
        assert_eq!(config.sample_rate, 48000);
        assert_eq!(config.channels, 2);
        assert_eq!(config.bits_per_sample, 16);
        assert_eq!(config.buffer_duration, Duration::from_millis(20));
        assert!(config.device_id.is_none());
    }
}
