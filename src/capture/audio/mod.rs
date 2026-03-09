//! WASAPI Audio Capture
//!
//! This module implements audio capture using Windows Audio Session API (WASAPI).
//!
//! # Features
//!
//! - **System Audio Capture**: Loopback mode captures all system audio
//! - **Microphone Capture**: Direct capture from microphone device
//! - **Audio Mixing**: Combines system and mic audio with volume control
//! - **Noise Suppression**: Optional RNNoise-based noise reduction
//!
//! # Architecture
//!
//! Audio capture runs on dedicated threads:
//!
//! 1. **System Audio Thread**: WASAPI loopback capture
//! 2. **Microphone Thread**: WASAPI capture device
//! 3. **Mixer Thread**: Combines and processes audio streams
//!
//! # Key Types
//!
//! - `AudioManager` - Orchestrates audio capture threads
//! - `SystemAudioCapture` - System audio loopback capture
//! - `MicCapture` - Microphone capture
//! - `AudioMixer` - Combines multiple audio streams
//!
//! # Example
//!
//! ```ignore
//! use liteclip_replay::capture::audio::AudioManager;
//! use liteclip_replay::config::AudioConfig;
//!
//! let mut manager = AudioManager::new();
//! manager.start(&audio_config)?;
//!
//! // Receive encoded audio packets
//! while let Ok(packet) = manager.packet_rx().recv() {
//!     buffer.push(packet);
//! }
//! ```

pub mod device_info;
pub mod manager;
pub mod mic;
pub mod mixer;
pub mod system;

pub use manager::WasapiAudioManager;
pub use mic::WasapiMicCapture;
pub use system::WasapiSystemCapture;
