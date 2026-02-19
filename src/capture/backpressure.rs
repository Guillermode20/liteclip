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
