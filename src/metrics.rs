//! Real-time FPS and performance metrics tracking.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Default)]
pub struct FpsMetrics {
    frames_this_second: AtomicU64,
    current_fps: AtomicU64,
    drops_this_second: AtomicU64,
    total_frames: AtomicU64,
    total_drops: AtomicU64,
    last_update: Mutex<Option<Instant>>,
}

impl FpsMetrics {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_frame(&self) {
        self.frames_this_second.fetch_add(1, Ordering::Relaxed);
        self.total_frames.fetch_add(1, Ordering::Relaxed);
        self.maybe_update_fps();
    }

    pub fn record_drop(&self) {
        self.drops_this_second.fetch_add(1, Ordering::Relaxed);
        self.total_drops.fetch_add(1, Ordering::Relaxed);
    }

    fn maybe_update_fps(&self) {
        if let Ok(mut last) = self.last_update.lock() {
            let now = Instant::now();
            let should_update = match *last {
                None => true,
                Some(t) => now.duration_since(t) >= Duration::from_secs(1),
            };

            if should_update {
                let fps = self.frames_this_second.swap(0, Ordering::Relaxed);
                self.current_fps.store(fps, Ordering::Relaxed);
                self.drops_this_second.store(0, Ordering::Relaxed);
                *last = Some(now);
            }
        }
    }

    pub fn current_fps(&self) -> u64 {
        self.current_fps.load(Ordering::Relaxed)
    }

    pub fn total_frames(&self) -> u64 {
        self.total_frames.load(Ordering::Relaxed)
    }

    pub fn total_drops(&self) -> u64 {
        self.total_drops.load(Ordering::Relaxed)
    }

    pub fn drop_rate(&self) -> f64 {
        let total = self.total_frames.load(Ordering::Relaxed);
        let drops = self.total_drops.load(Ordering::Relaxed);
        if total > 0 {
            drops as f64 / total as f64
        } else {
            0.0
        }
    }
}

#[derive(Debug, Clone)]
pub struct MetricsSnapshot {
    pub current_fps: u64,
    pub total_frames: u64,
    pub total_drops: u64,
    pub drop_rate: f64,
}

impl FpsMetrics {
    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            current_fps: self.current_fps(),
            total_frames: self.total_frames(),
            total_drops: self.total_drops(),
            drop_rate: self.drop_rate(),
        }
    }
}
