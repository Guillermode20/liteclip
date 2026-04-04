//! Audio Level Monitor
//!
//! Provides real-time audio level monitoring for visualization in the GUI.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

const DECAY_FACTOR: f32 = 0.92;
const SMOOTH_ALPHA: f32 = 0.4;
const METER_FLOOR_DB: f32 = -60.0;
const METER_DB_RANGE: f32 = 60.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AudioLevels {
    pub level: u8,
    pub peak: u8,
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
    /// Set to `true` while the settings GUI is open.
    /// When `false`, level calculations are skipped to save CPU.
    gui_active: std::sync::atomic::AtomicBool,
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
                gui_active: std::sync::atomic::AtomicBool::new(false),
            }),
        }
    }

    /// Notify the monitor whether the audio level GUI (settings window) is open.
    ///
    /// When `false` (default), `update_system_levels` and `update_mic_levels` are
    /// no-ops so the per-buffer RMS scan is skipped entirely.
    pub fn set_gui_active(&self, active: bool) {
        self.inner.gui_active.store(active, Ordering::Relaxed);
    }

    pub fn update_system_levels(&self, left: f32, right: f32) {
        // Skip heavy RMS computation when the GUI meter is not visible.
        if !self.inner.gui_active.load(Ordering::Relaxed) {
            return;
        }
        let combined = left.max(right).clamp(0.0, 1.0);
        let level_u32 = Self::amplitude_to_meter_level(combined);

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
        // Skip heavy RMS computation when the GUI meter is not visible.
        if !self.inner.gui_active.load(Ordering::Relaxed) {
            return;
        }
        let combined = left.max(right).clamp(0.0, 1.0);
        let level_u32 = Self::amplitude_to_meter_level(combined);

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
            let decayed = (current as f32 * DECAY_FACTOR) as u32;
            val.store(decayed, Ordering::Relaxed);
        };

        decay(&self.inner.system_peak);
        decay(&self.inner.mic_peak);
    }

    pub fn get_system_levels(&self) -> AudioLevels {
        AudioLevels {
            level: self.inner.system_level.load(Ordering::Relaxed) as u8,
            peak: self.inner.system_peak.load(Ordering::Relaxed) as u8,
        }
    }

    pub fn get_mic_levels(&self) -> AudioLevels {
        AudioLevels {
            level: self.inner.mic_level.load(Ordering::Relaxed) as u8,
            peak: self.inner.mic_peak.load(Ordering::Relaxed) as u8,
        }
    }

    #[inline]
    fn amplitude_to_meter_level(level: f32) -> u32 {
        if level <= 0.0 {
            return 0;
        }

        let db = 20.0 * level.max(0.000_01).log10();
        let normalized = ((db - METER_FLOOR_DB) / METER_DB_RANGE).clamp(0.0, 1.0);
        (normalized * 100.0).round() as u32
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

pub fn calculate_levels_stereo_bytes(samples: &[u8]) -> (f32, f32) {
    let mut chunks = samples.chunks_exact(4);
    if chunks.len() == 0 {
        return (0.0, 0.0);
    }

    let mut sum_left: f64 = 0.0;
    let mut sum_right: f64 = 0.0;
    let mut count: usize = 0;

    for chunk in &mut chunks {
        let left = i16::from_le_bytes([chunk[0], chunk[1]]);
        let right = i16::from_le_bytes([chunk[2], chunk[3]]);
        sum_left += (left as f64).powi(2);
        sum_right += (right as f64).powi(2);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mic_meter_shows_quiet_voice() {
        let monitor = AudioLevelMonitor::new();
        monitor.set_gui_active(true);

        monitor.update_mic_levels(0.0, 0.01);

        let levels = monitor.get_mic_levels();
        assert!(
            levels.level > 10,
            "expected visible meter movement, got {}",
            levels.level
        );
    }

    #[test]
    fn test_peak_decay_reaches_zero() {
        let monitor = AudioLevelMonitor::new();

        monitor.update_system_levels(0.0, 1.0);
        for _ in 0..200 {
            monitor.decay_peak_levels();
        }

        let levels = monitor.get_system_levels();
        assert_eq!(levels.peak, 0);
    }

    #[test]
    fn test_calculate_levels_stereo_bytes_matches_samples() {
        let samples = [1000i16, -2000, 3000, -4000, 5000, -6000, 7000, -8000];
        let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();

        let from_samples = calculate_levels_stereo(&samples);
        let from_bytes = calculate_levels_stereo_bytes(&bytes);

        assert!((from_samples.0 - from_bytes.0).abs() < 0.0001);
        assert!((from_samples.1 - from_bytes.1).abs() < 0.0001);
    }
}
