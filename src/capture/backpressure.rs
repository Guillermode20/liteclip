//! Backpressure signaling between capture and encoder threads.
//!
//! Provides atomic state for coordinating frame dropping and FPS adaptation
//! when the encoder cannot keep up with capture rate.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

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

    pub fn should_drop_frame(&self) -> bool {
        self.queued_frames.load(Ordering::Relaxed) >= self.max_queued_frames.load(Ordering::Relaxed)
    }

    pub fn on_frame_queued(&self) {
        self.queued_frames.fetch_add(1, Ordering::Relaxed);
    }

    pub fn on_frame_processed(&self) {
        self.queued_frames.fetch_sub(1, Ordering::Relaxed);
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

    pub fn queued_count(&self) -> u32 {
        self.queued_frames.load(Ordering::Relaxed)
    }
}

impl Default for BackpressureState {
    fn default() -> Self {
        Self::new()
    }
}

pub type SharedBackpressure = Arc<BackpressureState>;

#[cfg(test)]
mod tests {
    use super::BackpressureState;

    #[test]
    fn drop_threshold_respects_max_queue() {
        let state = BackpressureState::new();
        state
            .max_queued_frames
            .store(2, std::sync::atomic::Ordering::Relaxed);

        assert!(!state.should_drop_frame());
        state.on_frame_queued();
        assert!(!state.should_drop_frame());
        state.on_frame_queued();
        assert!(state.should_drop_frame());
    }

    #[test]
    fn queued_count_tracks_queue_events() {
        let state = BackpressureState::new();
        state.on_frame_queued();
        state.on_frame_queued();
        assert_eq!(state.queued_count(), 2);

        state.on_frame_processed();
        assert_eq!(state.queued_count(), 1);
    }

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
