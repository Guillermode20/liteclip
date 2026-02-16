//! WASAPI Audio Capture Module
//!
//! Implements system audio capture using WASAPI loopback mode
//! and microphone capture using WASAPI capture mode.

pub mod manager;
pub mod mic;
pub mod mixer;
pub mod system;

pub use manager::WasapiAudioManager;
pub use mic::WasapiMicCapture;
pub use mixer::AudioMixer;
pub use system::WasapiSystemCapture;
