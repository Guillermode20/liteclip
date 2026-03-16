//! Audio Level Monitor
//!
//! Provides real-time audio level monitoring for visualization in the GUI.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

const DECAY_FACTOR: f32 = 0.92;
const SMOOTH_ALPHA: f32 = 0.4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioLevels {
    pub level: u8,
    pub peak: u8,
}

impl Default for AudioLevels {
    fn default() -> Self {
        Self { level: 0, peak: 0 }
    }
}

#[derive(Debug, Clone)]
pub struct AudioLevelMonitor {
    inner: Arc<AudioLevelMonitorInner>,
}

#[derive(Debug)]
struct AudioLevelMonitorInner {
    system_level: AtomicU32,
    system_peak: AtomicU32,
    system_smoothed: AtomicU32,
    mic_level: AtomicU32,
    mic_peak: AtomicU32,
    mic_smoothed: AtomicU32,
}

impl Default for AudioLevelMonitor {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioLevelMonitor {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(AudioLevelMonitorInner {
                system_level: AtomicU32::new(0),
                system_peak: AtomicU32::new(0),
                system_smoothed: AtomicU32::new(0),
                mic_level: AtomicU32::new(0),
                mic_peak: AtomicU32::new(0),
                mic_smoothed: AtomicU32::new(0),
            }),
        }
    }

    pub fn update_system_levels(&self, left: f32, right: f32) {
        let combined = ((left + right) / 2.0).clamp(0.0, 1.0);
        let level_u32 = (combined * 1000.0) as u32;

        let smoothed = self.inner.system_smoothed.load(Ordering::Relaxed);
        let new_smoothed =
            ((smoothed as f32 * (1.0 - SMOOTH_ALPHA)) + (level_u32 as f32 * SMOOTH_ALPHA)) as u32;
        self.inner
            .system_smoothed
            .store(new_smoothed, Ordering::Relaxed);

        self.inner
            .system_level
            .store(new_smoothed, Ordering::Relaxed);

        let peak = self.inner.system_peak.load(Ordering::Relaxed);
        if new_smoothed > peak {
            self.inner
                .system_peak
                .store(new_smoothed, Ordering::Relaxed);
        }
    }

    pub fn update_mic_levels(&self, left: f32, right: f32) {
        let combined = ((left + right) / 2.0).clamp(0.0, 1.0);
        let level_u32 = (combined * 1000.0) as u32;

        let smoothed = self.inner.mic_smoothed.load(Ordering::Relaxed);
        let new_smoothed =
            ((smoothed as f32 * (1.0 - SMOOTH_ALPHA)) + (level_u32 as f32 * SMOOTH_ALPHA)) as u32;
        self.inner
            .mic_smoothed
            .store(new_smoothed, Ordering::Relaxed);

        self.inner.mic_level.store(new_smoothed, Ordering::Relaxed);

        let peak = self.inner.mic_peak.load(Ordering::Relaxed);
        if new_smoothed > peak {
            self.inner.mic_peak.store(new_smoothed, Ordering::Relaxed);
        }
    }

    pub fn decay_peak_levels(&self) {
        let decay = |val: &AtomicU32| {
            let current = val.load(Ordering::Relaxed);
            let decayed = ((current as f32 * DECAY_FACTOR) as u32).max(1);
            val.store(decayed, Ordering::Relaxed);
        };

        decay(&self.inner.system_peak);
        decay(&self.inner.mic_peak);
    }

    pub fn get_system_levels(&self) -> AudioLevels {
        AudioLevels {
            level: (self.inner.system_level.load(Ordering::Relaxed) / 10) as u8,
            peak: (self.inner.system_peak.load(Ordering::Relaxed) / 10) as u8,
        }
    }

    pub fn get_mic_levels(&self) -> AudioLevels {
        AudioLevels {
            level: (self.inner.mic_level.load(Ordering::Relaxed) / 10) as u8,
            peak: (self.inner.mic_peak.load(Ordering::Relaxed) / 10) as u8,
        }
    }
}

pub fn calculate_levels_stereo(samples: &[i16]) -> (f32, f32) {
    if samples.is_empty() {
        return (0.0, 0.0);
    }

    let mut sum_left: f64 = 0.0;
    let mut sum_right: f64 = 0.0;
    let mut count: usize = 0;

    for chunk in samples.chunks_exact(2) {
        sum_left += (chunk[0] as f64).powi(2);
        sum_right += (chunk[1] as f64).powi(2);
        count += 1;
    }

    if count == 0 {
        return (0.0, 0.0);
    }

    let rms_left = (sum_left / count as f64).sqrt();
    let rms_right = (sum_right / count as f64).sqrt();

    let max_val = 32768.0f64;

    let level_left = rms_left / max_val;
    let level_right = rms_right / max_val;

    (level_left.min(1.0) as f32, level_right.min(1.0) as f32)
}

pub fn calculate_levels_mono(samples: &[i16]) -> (f32, f32) {
    if samples.is_empty() {
        return (0.0, 0.0);
    }

    let mut sum: f64 = 0.0;
    for &sample in samples {
        sum += (sample as f64).powi(2);
    }

    let rms = (sum / samples.len() as f64).sqrt();
    let level = (rms / 32768.0).min(1.0) as f32;

    (level, level)
}
