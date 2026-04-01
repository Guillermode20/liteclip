//! Stable quality/performance contracts for no-regression work.
//!
//! These contracts intentionally live in code (not docs-only) so optimization phases can
//! evaluate CPU/memory improvements without silently regressing user-visible quality.

use crate::output::VideoFileMetadata;
use std::fmt;

/// Canonical performance scenarios used for profiling and regression comparisons.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CanonicalPerformanceScenario {
    IdleTray,
    ActiveRecording,
    SaveBurst,
    GalleryPlaybackScrub,
    Export,
}

impl CanonicalPerformanceScenario {
    /// Stable scenario identifier used by benchmark harness tooling and serialized summaries.
    pub const fn as_slug(self) -> &'static str {
        match self {
            CanonicalPerformanceScenario::IdleTray => "idle-tray",
            CanonicalPerformanceScenario::ActiveRecording => "active-recording",
            CanonicalPerformanceScenario::SaveBurst => "save-burst",
            CanonicalPerformanceScenario::GalleryPlaybackScrub => "gallery-playback-scrub",
            CanonicalPerformanceScenario::Export => "export",
        }
    }

    /// Parses a scenario identifier accepted by benchmark harness tooling.
    pub fn from_slug(value: &str) -> Option<Self> {
        let normalized = value.trim().to_ascii_lowercase();
        match normalized.as_str() {
            "idle-tray" | "idle_tray" | "idletray" => Some(CanonicalPerformanceScenario::IdleTray),
            "active-recording" | "active_recording" | "activerecording" => {
                Some(CanonicalPerformanceScenario::ActiveRecording)
            }
            "save-burst" | "save_burst" | "saveburst" => {
                Some(CanonicalPerformanceScenario::SaveBurst)
            }
            "gallery-playback-scrub"
            | "gallery_playback_scrub"
            | "galleryplaybackscrub"
            | "gallery-playback"
            | "gallery" => Some(CanonicalPerformanceScenario::GalleryPlaybackScrub),
            "export" => Some(CanonicalPerformanceScenario::Export),
            _ => None,
        }
    }
}

impl fmt::Display for CanonicalPerformanceScenario {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str((*self).as_slug())
    }
}

/// High-level runtime area covered by a scenario.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScenarioScope {
    CoreRecordingPipeline,
    ExportPipeline,
    GalleryRuntime,
}

impl ScenarioScope {
    /// Stable label used in serialized benchmark summaries.
    pub const fn as_str(self) -> &'static str {
        match self {
            ScenarioScope::CoreRecordingPipeline => "core-recording-pipeline",
            ScenarioScope::ExportPipeline => "export-pipeline",
            ScenarioScope::GalleryRuntime => "gallery-runtime",
        }
    }
}

/// Resource objective for a benchmark scenario.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourcePriority {
    /// CPU and memory are treated as equally important.
    BalancedCpuMemory,
}

impl ResourcePriority {
    /// Stable label used in serialized benchmark summaries.
    pub const fn as_str(self) -> &'static str {
        match self {
            ResourcePriority::BalancedCpuMemory => "balanced-cpu-memory",
        }
    }
}

/// Quality checks that must be preserved while tuning performance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QualityGuardrailKind {
    AvSync,
    DroppedFrames,
    ExportValidity,
}

impl QualityGuardrailKind {
    /// Stable label used in serialized benchmark summaries.
    pub const fn as_str(self) -> &'static str {
        match self {
            QualityGuardrailKind::AvSync => "av-sync",
            QualityGuardrailKind::DroppedFrames => "dropped-frames",
            QualityGuardrailKind::ExportValidity => "export-validity",
        }
    }
}

/// Canonical scenario contract consumed by performance harnesses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PerformanceScenarioContract {
    pub id: CanonicalPerformanceScenario,
    pub scope: ScenarioScope,
    pub priority: ResourcePriority,
    pub warmup_secs: u32,
    pub sample_secs: u32,
    pub quality_guardrails: &'static [QualityGuardrailKind],
    pub description: &'static str,
}

const IDLE_GUARDRAILS: [QualityGuardrailKind; 0] = [];
const RECORDING_GUARDRAILS: [QualityGuardrailKind; 2] = [
    QualityGuardrailKind::AvSync,
    QualityGuardrailKind::DroppedFrames,
];
const SAVE_BURST_GUARDRAILS: [QualityGuardrailKind; 2] = [
    QualityGuardrailKind::DroppedFrames,
    QualityGuardrailKind::ExportValidity,
];
const GALLERY_GUARDRAILS: [QualityGuardrailKind; 2] = [
    QualityGuardrailKind::AvSync,
    QualityGuardrailKind::DroppedFrames,
];
const EXPORT_GUARDRAILS: [QualityGuardrailKind; 1] = [QualityGuardrailKind::ExportValidity];

/// Canonical scenarios used for repeatable CPU/memory comparisons.
pub static CANONICAL_PERFORMANCE_SCENARIOS: [PerformanceScenarioContract; 5] = [
    PerformanceScenarioContract {
        id: CanonicalPerformanceScenario::IdleTray,
        scope: ScenarioScope::CoreRecordingPipeline,
        priority: ResourcePriority::BalancedCpuMemory,
        warmup_secs: 20,
        sample_secs: 120,
        quality_guardrails: &IDLE_GUARDRAILS,
        description: "Tray-only idle runtime with no active recording or gallery activity.",
    },
    PerformanceScenarioContract {
        id: CanonicalPerformanceScenario::ActiveRecording,
        scope: ScenarioScope::CoreRecordingPipeline,
        priority: ResourcePriority::BalancedCpuMemory,
        warmup_secs: 15,
        sample_secs: 180,
        quality_guardrails: &RECORDING_GUARDRAILS,
        description: "Continuous capture + encode + replay buffering at configured user quality.",
    },
    PerformanceScenarioContract {
        id: CanonicalPerformanceScenario::SaveBurst,
        scope: ScenarioScope::CoreRecordingPipeline,
        priority: ResourcePriority::BalancedCpuMemory,
        warmup_secs: 10,
        sample_secs: 90,
        quality_guardrails: &SAVE_BURST_GUARDRAILS,
        description: "Recording remains active while multiple clips are saved in short succession.",
    },
    PerformanceScenarioContract {
        id: CanonicalPerformanceScenario::GalleryPlaybackScrub,
        scope: ScenarioScope::GalleryRuntime,
        priority: ResourcePriority::BalancedCpuMemory,
        warmup_secs: 10,
        sample_secs: 120,
        quality_guardrails: &GALLERY_GUARDRAILS,
        description: "Gallery playback and repeated timeline scrubbing in the editor/runtime view.",
    },
    PerformanceScenarioContract {
        id: CanonicalPerformanceScenario::Export,
        scope: ScenarioScope::ExportPipeline,
        priority: ResourcePriority::BalancedCpuMemory,
        warmup_secs: 0,
        sample_secs: 60,
        quality_guardrails: &EXPORT_GUARDRAILS,
        description: "Trimmed clip export workflow from gallery editor to encoded output file.",
    },
];

/// Returns the canonical contract associated with a benchmark scenario.
pub const fn canonical_scenario_contract(
    scenario: CanonicalPerformanceScenario,
) -> &'static PerformanceScenarioContract {
    match scenario {
        CanonicalPerformanceScenario::IdleTray => &CANONICAL_PERFORMANCE_SCENARIOS[0],
        CanonicalPerformanceScenario::ActiveRecording => &CANONICAL_PERFORMANCE_SCENARIOS[1],
        CanonicalPerformanceScenario::SaveBurst => &CANONICAL_PERFORMANCE_SCENARIOS[2],
        CanonicalPerformanceScenario::GalleryPlaybackScrub => &CANONICAL_PERFORMANCE_SCENARIOS[3],
        CanonicalPerformanceScenario::Export => &CANONICAL_PERFORMANCE_SCENARIOS[4],
    }
}

/// A/V sync tolerance for quality regression checks.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AvSyncGuardrail {
    /// Absolute allowed skew between audio and video timelines.
    pub max_abs_desync_ms: f64,
}

/// Default A/V sync contract for recording/playback checks.
pub const AV_SYNC_GUARDRAIL: AvSyncGuardrail = AvSyncGuardrail {
    max_abs_desync_ms: 120.0,
};

/// Capture drop tolerance expectations for the active recording scenario.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DroppedFrameGuardrail {
    pub sample_window_secs: u32,
    pub max_drop_ratio: f64,
    pub max_fps_divisor: u32,
}

/// Active recording contract (30s windows align with existing capture telemetry cadence).
pub const ACTIVE_RECORDING_DROPPED_FRAMES_GUARDRAIL: DroppedFrameGuardrail =
    DroppedFrameGuardrail {
        sample_window_secs: 30,
        max_drop_ratio: 0.10,
        max_fps_divisor: 3,
    };

/// Runtime sample for evaluating recording drop behavior.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ActiveRecordingWindowSample {
    pub captured_frames: u64,
    pub dropped_frames: u64,
    pub fps_divisor: u32,
}

/// Result of evaluating an active-recording drop sample.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ActiveRecordingQualityAssessment {
    pub drop_ratio: f64,
    pub within_drop_ratio: bool,
    pub within_fps_divisor: bool,
    pub within_contract: bool,
}

/// Gallery playback/scrub quality guardrail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GalleryPlaybackGuardrail {
    pub max_stale_frames_dropped_per_poll: u32,
    pub max_empty_queue_polls: u64,
    pub max_queue_depth_frames: usize,
}

/// Contract tuned to current gallery playback queue behavior.
pub const GALLERY_PLAYBACK_GUARDRAIL: GalleryPlaybackGuardrail = GalleryPlaybackGuardrail {
    max_stale_frames_dropped_per_poll: 6,
    max_empty_queue_polls: 180,
    max_queue_depth_frames: 20,
};

/// Runtime sample for gallery playback quality signals.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GalleryPlaybackRuntimeSample {
    pub stale_frames_dropped: u32,
    pub empty_queue_polls: u64,
    pub queue_depth: usize,
}

/// Result of evaluating gallery runtime quality signals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GalleryPlaybackAssessment {
    pub within_stale_drop_limit: bool,
    pub within_empty_queue_limit: bool,
    pub within_queue_depth_limit: bool,
    pub within_contract: bool,
}

/// Export validity contract used after clip export/probe.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ExportValidityGuardrail {
    pub min_duration_ratio: f64,
    pub max_duration_overrun_secs: f64,
    pub min_fps: f64,
    pub max_fps: f64,
}

/// Default export validity thresholds.
pub const EXPORT_VALIDITY_GUARDRAIL: ExportValidityGuardrail = ExportValidityGuardrail {
    min_duration_ratio: 0.97,
    max_duration_overrun_secs: 0.75,
    min_fps: 1.0,
    max_fps: 240.0,
};

/// Input for export validity checks.
#[derive(Debug, Clone, Copy)]
pub struct ExportValidationInput<'a> {
    pub expected_duration_secs: f64,
    pub expect_audio: bool,
    pub metadata: &'a VideoFileMetadata,
}

/// Export validity violations.
#[derive(Debug, Clone, PartialEq)]
pub enum ExportValidityViolation {
    NonFiniteDuration {
        duration_secs: f64,
    },
    NonPositiveDuration {
        duration_secs: f64,
    },
    DurationTooShort {
        actual_secs: f64,
        minimum_secs: f64,
    },
    DurationTooLong {
        actual_secs: f64,
        maximum_secs: f64,
    },
    MissingAudioTrack,
    InvalidResolution {
        width: u32,
        height: u32,
    },
    NonFiniteFps {
        fps: f64,
    },
    InvalidFps {
        fps: f64,
        min_fps: f64,
        max_fps: f64,
    },
}

impl fmt::Display for ExportValidityViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExportValidityViolation::NonFiniteDuration { duration_secs } => {
                write!(f, "duration is not finite ({duration_secs})")
            }
            ExportValidityViolation::NonPositiveDuration { duration_secs } => {
                write!(f, "duration is non-positive ({duration_secs:.3}s)")
            }
            ExportValidityViolation::DurationTooShort {
                actual_secs,
                minimum_secs,
            } => write!(
                f,
                "duration too short ({actual_secs:.3}s < minimum {minimum_secs:.3}s)"
            ),
            ExportValidityViolation::DurationTooLong {
                actual_secs,
                maximum_secs,
            } => write!(
                f,
                "duration too long ({actual_secs:.3}s > maximum {maximum_secs:.3}s)"
            ),
            ExportValidityViolation::MissingAudioTrack => {
                write!(f, "audio track missing while audio was expected")
            }
            ExportValidityViolation::InvalidResolution { width, height } => {
                write!(f, "invalid resolution {width}x{height}")
            }
            ExportValidityViolation::NonFiniteFps { fps } => {
                write!(f, "fps is not finite ({fps})")
            }
            ExportValidityViolation::InvalidFps {
                fps,
                min_fps,
                max_fps,
            } => write!(
                f,
                "fps out of range ({fps:.3} not in {min_fps:.1}..={max_fps:.1})"
            ),
        }
    }
}

/// Signed A/V sync skew in milliseconds (audio time minus video time).
pub fn av_desync_ms(audio_pts_qpc: i64, video_pts_qpc: i64, qpc_frequency: i64) -> f64 {
    let freq = qpc_frequency.max(1) as f64;
    (audio_pts_qpc.saturating_sub(video_pts_qpc) as f64) * 1000.0 / freq
}

/// Checks if signed A/V skew is within [`AV_SYNC_GUARDRAIL`].
pub fn is_av_sync_within_guardrail(signed_desync_ms: f64) -> bool {
    signed_desync_ms.is_finite() && signed_desync_ms.abs() <= AV_SYNC_GUARDRAIL.max_abs_desync_ms
}

/// Evaluates dropped-frame quality for an active recording telemetry window.
pub fn assess_active_recording_window(
    sample: ActiveRecordingWindowSample,
) -> ActiveRecordingQualityAssessment {
    let total_frames = sample.captured_frames.saturating_add(sample.dropped_frames);
    let drop_ratio = if total_frames == 0 {
        0.0
    } else {
        sample.dropped_frames as f64 / total_frames as f64
    };
    let within_drop_ratio = drop_ratio <= ACTIVE_RECORDING_DROPPED_FRAMES_GUARDRAIL.max_drop_ratio;
    let within_fps_divisor =
        sample.fps_divisor <= ACTIVE_RECORDING_DROPPED_FRAMES_GUARDRAIL.max_fps_divisor;
    ActiveRecordingQualityAssessment {
        drop_ratio,
        within_drop_ratio,
        within_fps_divisor,
        within_contract: within_drop_ratio && within_fps_divisor,
    }
}

/// Evaluates gallery playback/scrub runtime quality indicators.
pub fn assess_gallery_playback_runtime(
    sample: GalleryPlaybackRuntimeSample,
) -> GalleryPlaybackAssessment {
    let within_stale_drop_limit =
        sample.stale_frames_dropped <= GALLERY_PLAYBACK_GUARDRAIL.max_stale_frames_dropped_per_poll;
    let within_empty_queue_limit =
        sample.empty_queue_polls <= GALLERY_PLAYBACK_GUARDRAIL.max_empty_queue_polls;
    let within_queue_depth_limit =
        sample.queue_depth <= GALLERY_PLAYBACK_GUARDRAIL.max_queue_depth_frames;
    GalleryPlaybackAssessment {
        within_stale_drop_limit,
        within_empty_queue_limit,
        within_queue_depth_limit,
        within_contract: within_stale_drop_limit
            && within_empty_queue_limit
            && within_queue_depth_limit,
    }
}

/// Validates exported file metadata against [`EXPORT_VALIDITY_GUARDRAIL`].
pub fn validate_export_validity(input: ExportValidationInput<'_>) -> Vec<ExportValidityViolation> {
    let mut violations = Vec::new();
    let metadata = input.metadata;

    if metadata.width == 0 || metadata.height == 0 {
        violations.push(ExportValidityViolation::InvalidResolution {
            width: metadata.width,
            height: metadata.height,
        });
    }

    if !metadata.duration_secs.is_finite() {
        violations.push(ExportValidityViolation::NonFiniteDuration {
            duration_secs: metadata.duration_secs,
        });
    } else if metadata.duration_secs <= 0.0 {
        violations.push(ExportValidityViolation::NonPositiveDuration {
            duration_secs: metadata.duration_secs,
        });
    } else if input.expected_duration_secs.is_finite() && input.expected_duration_secs > 0.0 {
        let min_duration =
            input.expected_duration_secs * EXPORT_VALIDITY_GUARDRAIL.min_duration_ratio;
        let max_duration =
            input.expected_duration_secs + EXPORT_VALIDITY_GUARDRAIL.max_duration_overrun_secs;
        if metadata.duration_secs < min_duration {
            violations.push(ExportValidityViolation::DurationTooShort {
                actual_secs: metadata.duration_secs,
                minimum_secs: min_duration,
            });
        }
        if metadata.duration_secs > max_duration {
            violations.push(ExportValidityViolation::DurationTooLong {
                actual_secs: metadata.duration_secs,
                maximum_secs: max_duration,
            });
        }
    }

    if input.expect_audio && !metadata.has_audio {
        violations.push(ExportValidityViolation::MissingAudioTrack);
    }

    if !metadata.fps.is_finite() {
        violations.push(ExportValidityViolation::NonFiniteFps { fps: metadata.fps });
    } else if metadata.fps < EXPORT_VALIDITY_GUARDRAIL.min_fps
        || metadata.fps > EXPORT_VALIDITY_GUARDRAIL.max_fps
    {
        violations.push(ExportValidityViolation::InvalidFps {
            fps: metadata.fps,
            min_fps: EXPORT_VALIDITY_GUARDRAIL.min_fps,
            max_fps: EXPORT_VALIDITY_GUARDRAIL.max_fps,
        });
    }

    violations
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn canonical_scenarios_are_unique_and_complete() {
        let ids: HashSet<_> = CANONICAL_PERFORMANCE_SCENARIOS
            .iter()
            .map(|scenario| scenario.id)
            .collect();
        assert_eq!(ids.len(), 5);
        assert!(ids.contains(&CanonicalPerformanceScenario::IdleTray));
        assert!(ids.contains(&CanonicalPerformanceScenario::ActiveRecording));
        assert!(ids.contains(&CanonicalPerformanceScenario::SaveBurst));
        assert!(ids.contains(&CanonicalPerformanceScenario::GalleryPlaybackScrub));
        assert!(ids.contains(&CanonicalPerformanceScenario::Export));
    }

    #[test]
    fn scenario_slugs_round_trip() {
        for scenario in [
            CanonicalPerformanceScenario::IdleTray,
            CanonicalPerformanceScenario::ActiveRecording,
            CanonicalPerformanceScenario::SaveBurst,
            CanonicalPerformanceScenario::GalleryPlaybackScrub,
            CanonicalPerformanceScenario::Export,
        ] {
            let slug = scenario.as_slug();
            assert_eq!(
                CanonicalPerformanceScenario::from_slug(slug),
                Some(scenario)
            );
            assert_eq!(canonical_scenario_contract(scenario).id, scenario);
        }
    }

    #[test]
    fn scenarios_use_balanced_cpu_memory_priority() {
        assert!(CANONICAL_PERFORMANCE_SCENARIOS
            .iter()
            .all(|scenario| scenario.priority == ResourcePriority::BalancedCpuMemory));
    }

    #[test]
    fn av_sync_guardrail_accepts_small_skew_and_rejects_large_skew() {
        assert!(is_av_sync_within_guardrail(95.0));
        assert!(!is_av_sync_within_guardrail(175.0));
    }

    #[test]
    fn active_recording_assessment_reports_drop_ratio_and_limits() {
        let ok = assess_active_recording_window(ActiveRecordingWindowSample {
            captured_frames: 900,
            dropped_frames: 60,
            fps_divisor: 1,
        });
        assert!(ok.within_contract);
        assert!(ok.drop_ratio < ACTIVE_RECORDING_DROPPED_FRAMES_GUARDRAIL.max_drop_ratio);

        let bad = assess_active_recording_window(ActiveRecordingWindowSample {
            captured_frames: 400,
            dropped_frames: 120,
            fps_divisor: 4,
        });
        assert!(!bad.within_contract);
        assert!(!bad.within_drop_ratio);
        assert!(!bad.within_fps_divisor);
    }

    #[test]
    fn gallery_assessment_enforces_runtime_limits() {
        let ok = assess_gallery_playback_runtime(GalleryPlaybackRuntimeSample {
            stale_frames_dropped: 4,
            empty_queue_polls: 120,
            queue_depth: 12,
        });
        assert!(ok.within_contract);

        let bad = assess_gallery_playback_runtime(GalleryPlaybackRuntimeSample {
            stale_frames_dropped: 10,
            empty_queue_polls: 250,
            queue_depth: 24,
        });
        assert!(!bad.within_contract);
        assert!(!bad.within_stale_drop_limit);
        assert!(!bad.within_empty_queue_limit);
        assert!(!bad.within_queue_depth_limit);
    }

    #[test]
    fn export_validation_detects_short_missing_audio_output() {
        let metadata = VideoFileMetadata {
            duration_secs: 9.0,
            width: 1920,
            height: 1080,
            has_audio: false,
            fps: 60.0,
        };
        let violations = validate_export_validity(ExportValidationInput {
            expected_duration_secs: 10.0,
            expect_audio: true,
            metadata: &metadata,
        });
        assert!(violations
            .iter()
            .any(|v| matches!(v, ExportValidityViolation::DurationTooShort { .. })));
        assert!(violations
            .iter()
            .any(|v| matches!(v, ExportValidityViolation::MissingAudioTrack)));
    }

    #[test]
    fn export_validation_accepts_nominal_output() {
        let metadata = VideoFileMetadata {
            duration_secs: 59.7,
            width: 1920,
            height: 1080,
            has_audio: true,
            fps: 59.94,
        };
        let violations = validate_export_validity(ExportValidationInput {
            expected_duration_secs: 60.0,
            expect_audio: true,
            metadata: &metadata,
        });
        assert!(violations.is_empty(), "violations: {violations:?}");
    }

    #[test]
    fn av_desync_uses_qpc_frequency() {
        let skew_ms = av_desync_ms(10_100_000, 10_000_000, 10_000_000);
        assert!((skew_ms - 10.0).abs() < f64::EPSILON);
    }
}
