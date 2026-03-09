//! Backpressure signaling between capture and encoder threads.
//!
//! Provides atomic state for coordinating frame dropping and FPS adaptation
//! when the encoder cannot keep up with capture rate.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

pub struct BackpressureState {
    pub queued_frames: AtomicU32,
    pub max_queued_frames: AtomicU32,
    pub encoder_overloaded: AtomicBool,
    pub fps_divisor: AtomicU32,
}

impl BackpressureState {
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
