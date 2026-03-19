//! Backpressure signaling between capture and encoder threads.
//!
//! Provides atomic state for coordinating frame dropping and FPS adaptation
//! when the encoder (GPU or CPU) cannot keep up with the frame acquisition rate.
//!
//! # Why it exists
//!
//! High-quality screen capture can generate large amounts of data. If the encoder
//! gets backed up, RAM usage will grow unbounded. This module signals the capture
//! loop to proactively drop frames (`fps_divisor`) or stop altogether if the
//! overload persists.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// Shared state for monitoring and responding to encoder saturation.
pub struct BackpressureState {
    /// Number of frames currently in the pipeline (captured but not encoded).
    pub queued_frames: AtomicU32,
    /// Maximum number of frames allowed in the pipeline before applying pressure.
    pub max_queued_frames: AtomicU32,
    /// Global flag indicating the encoder is consistently falling behind.
    pub encoder_overloaded: AtomicBool,
    /// When greater than 1, capture loop will only process 1 out of every `N` frames.
    /// This effectively reduces capture FPS to prevent stutter and RAM growth.
    pub fps_divisor: AtomicU32,
}

impl BackpressureState {
    /// Creates a new backpressure tracker with default limits.
    pub fn new() -> Self {
        Self {
            queued_frames: AtomicU32::new(0),
            max_queued_frames: AtomicU32::new(8),
            encoder_overloaded: AtomicBool::new(false),
            fps_divisor: AtomicU32::new(0),
        }
    }

    pub fn current_fps_divisor(&self) -> u32 {
        self.fps_divisor.load(Ordering::Relaxed)
    }

    pub fn set_fps_divisor(&self, divisor: u32) {
        self.fps_divisor.store(divisor, Ordering::Relaxed);
    }

    pub fn is_encoder_overloaded(&self) -> bool {
        self.encoder_overloaded.load(Ordering::Relaxed)
    }

    pub fn set_encoder_overloaded(&self, overloaded: bool) {
        self.encoder_overloaded.store(overloaded, Ordering::Relaxed);
    }
}

impl Default for BackpressureState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::BackpressureState;

    #[test]
    fn fps_divisor_round_trip() {
        let state = BackpressureState::new();
        assert_eq!(state.current_fps_divisor(), 0);

        state.set_fps_divisor(3);
        assert_eq!(state.current_fps_divisor(), 3);
    }

    #[test]
    fn overload_flag_round_trip() {
        let state = BackpressureState::new();
        assert!(!state.is_encoder_overloaded());

        state.set_encoder_overloaded(true);
        assert!(state.is_encoder_overloaded());

        state.set_encoder_overloaded(false);
        assert!(!state.is_encoder_overloaded());
    }
}
