//! Runtime budget governor for coordinating queue pressure and adaptive capture policy.
//!
//! This centralizes flow-control tuning so capture/encode behavior follows one policy surface
//! instead of ad-hoc thresholds scattered across loops.

/// Snapshot of runtime pressure signals observed in capture loop.
#[derive(Debug, Clone, Copy)]
pub struct RuntimeBudgetInput {
    pub queue_len: u32,
    pub queue_cap: u32,
    pub encoder_overloaded: bool,
}

/// Policy output for the capture loop.
#[derive(Debug, Clone, Copy)]
pub struct RuntimeBudgetDecision {
    pub high_watermark: u32,
    pub low_watermark: u32,
    pub severe_watermark: u32,
    pub max_fps_divisor: u32,
    pub high_streak_threshold: u32,
    pub low_streak_threshold: u32,
}

/// Stateful governor; designed to be cheap to evaluate in hot loops.
#[derive(Debug, Clone, Copy)]
pub struct RuntimeBudgetGovernor {
    max_fps_divisor: u32,
    high_streak_threshold: u32,
    low_streak_threshold: u32,
}

impl RuntimeBudgetGovernor {
    pub fn new() -> Self {
        Self {
            max_fps_divisor: 3,
            high_streak_threshold: 2,
            low_streak_threshold: 3,
        }
    }

    pub fn decide(&self, input: RuntimeBudgetInput) -> RuntimeBudgetDecision {
        let queue_cap = input.queue_cap.max(1);
        let mut high_watermark = queue_cap.saturating_mul(3) / 4;
        let mut low_watermark = queue_cap / 4;
        let mut severe_watermark = queue_cap.saturating_mul(7) / 8;

        if input.encoder_overloaded {
            high_watermark = high_watermark.min(queue_cap.saturating_mul(2) / 3).max(1);
            severe_watermark = severe_watermark.min(queue_cap.saturating_mul(4) / 5).max(1);
            low_watermark = low_watermark.min(high_watermark.saturating_sub(1));
        }

        if input.queue_len >= severe_watermark {
            low_watermark = low_watermark.min(severe_watermark.saturating_sub(1));
            high_watermark = high_watermark.min(severe_watermark);
        }

        RuntimeBudgetDecision {
            high_watermark,
            low_watermark,
            severe_watermark,
            max_fps_divisor: self.max_fps_divisor,
            high_streak_threshold: self.high_streak_threshold,
            low_streak_threshold: self.low_streak_threshold,
        }
    }
}

impl Default for RuntimeBudgetGovernor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overloaded_input_tightens_watermarks() {
        let governor = RuntimeBudgetGovernor::new();
        let nominal = governor.decide(RuntimeBudgetInput {
            queue_len: 0,
            queue_cap: 32,
            encoder_overloaded: false,
        });
        let overloaded = governor.decide(RuntimeBudgetInput {
            queue_len: 0,
            queue_cap: 32,
            encoder_overloaded: true,
        });

        assert!(overloaded.high_watermark <= nominal.high_watermark);
        assert!(overloaded.severe_watermark <= nominal.severe_watermark);
    }
}
