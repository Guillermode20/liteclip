//! Scenario-driven benchmark harness over existing runtime telemetry.
//!
//! This module parses telemetry lines already emitted by the runtime (capture/audio/pipeline/encoder)
//! and produces stable JSON summaries suitable for baseline vs post-change comparisons.
//! It intentionally avoids adding new heavy instrumentation and instead reuses existing log signals.

use crate::quality_contracts::{
    self, canonical_scenario_contract, CanonicalPerformanceScenario, PerformanceScenarioContract,
    QualityGuardrailKind,
};
use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// A single benchmark scenario run input.
#[derive(Debug, Clone)]
pub struct BenchmarkRunInput {
    pub label: String,
    pub scenario: CanonicalPerformanceScenario,
    pub stdout_log: PathBuf,
    pub stderr_log: Option<PathBuf>,
}

/// Options controlling benchmark suite summarization behavior.
#[derive(Debug, Clone)]
pub struct BenchmarkSuiteOptions {
    pub run_label: String,
    pub compare_to_label: Option<String>,
    pub strict_quality_guardrails: bool,
}

impl BenchmarkSuiteOptions {
    /// Default options for a single-run summary.
    pub fn single_run(run_label: impl Into<String>) -> Self {
        Self {
            run_label: run_label.into(),
            compare_to_label: None,
            strict_quality_guardrails: false,
        }
    }
}

/// Benchmark suite result across one or more canonical scenario logs.
#[derive(Debug, Clone)]
pub struct BenchmarkSuiteSummary {
    pub run_label: String,
    pub compare_to_label: Option<String>,
    pub generated_by: &'static str,
    pub scenarios: Vec<ScenarioSummary>,
    pub overall: OverallSummary,
}

/// Summary for one scenario run.
#[derive(Debug, Clone)]
pub struct ScenarioSummary {
    pub scenario: CanonicalPerformanceScenario,
    pub contract: PerformanceScenarioContract,
    pub run_label: String,
    pub stdout_log: PathBuf,
    pub stderr_log: Option<PathBuf>,
    pub telemetry_windows: TelemetryWindowCounts,
    pub metrics: ScenarioMetrics,
    pub quality: ScenarioQualitySummary,
    pub warnings: Vec<String>,
}

/// Counts of parsed telemetry windows by subsystem.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TelemetryWindowCounts {
    pub capture_windows: usize,
    pub audio_windows: usize,
    pub pipeline_windows: usize,
    pub encoder_windows: usize,
}

/// Aggregated scenario metrics from parsed telemetry windows.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ScenarioMetrics {
    pub capture_fps: NumericSummary,
    pub capture_drop_ratio: NumericSummary,
    pub capture_divisor: NumericSummary,
    pub capture_duplicates: NumericSummary,
    pub audio_pending_system: NumericSummary,
    pub audio_pending_mic: NumericSummary,
    pub pipeline_buffer_mb: NumericSummary,
    pub pipeline_buffer_usage_pct: NumericSummary,
    pub encoder_buffer_mb: NumericSummary,
    pub encoder_buffer_usage_pct: NumericSummary,
    pub encoder_pinned_snapshots_mb: NumericSummary,
    pub process_working_set_mb: NumericSummary,
    pub process_private_mb: NumericSummary,
}

/// Quality guardrail evaluation for one scenario.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScenarioQualitySummary {
    pub required_guardrails: Vec<QualityGuardrailKind>,
    pub evaluations: Vec<GuardrailEvaluation>,
    pub all_required_guardrails_passed: bool,
}

/// Evaluation result for a specific guardrail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuardrailEvaluation {
    pub guardrail: QualityGuardrailKind,
    pub status: GuardrailStatus,
    pub detail: String,
}

/// Outcome of evaluating a quality guardrail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardrailStatus {
    Pass,
    Fail,
    MissingTelemetry,
    NotApplicable,
}

/// Suite-level aggregate status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverallSummary {
    pub total_scenarios: usize,
    pub scenarios_with_quality_pass: usize,
    pub scenarios_with_missing_telemetry: usize,
    pub strict_quality_guardrails: bool,
}

/// Numeric summary stable enough for baseline/post-change comparisons.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NumericSummary {
    pub samples: usize,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub mean: Option<f64>,
    pub p50: Option<f64>,
    pub p95: Option<f64>,
}

/// Summarization failure reasons.
#[derive(Debug, thiserror::Error)]
pub enum BenchmarkHarnessError {
    #[error("failed to read log file '{path}': {source}")]
    ReadLog {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("scenario '{scenario}' did not produce required telemetry: {detail}")]
    MissingRequiredTelemetry {
        scenario: CanonicalPerformanceScenario,
        detail: String,
    },
    #[error("input error: {0}")]
    Input(String),
}

#[derive(Debug, Clone, Default)]
struct ParsedTelemetry {
    capture: Vec<CaptureWindow>,
    audio: Vec<AudioWindow>,
    pipeline: Vec<PipelineWindow>,
    encoder: Vec<EncoderWindow>,
}

#[derive(Debug, Clone, Copy)]
struct CaptureWindow {
    fps: f64,
    drop_ratio: f64,
    duplicates: f64,
    divisor: f64,
    quality_contract_ok: Option<bool>,
    process_working_set_mb: Option<f64>,
    process_private_mb: Option<f64>,
}

#[derive(Debug, Clone, Copy)]
struct AudioWindow {
    pending_system: f64,
    pending_mic: f64,
    process_working_set_mb: Option<f64>,
    process_private_mb: Option<f64>,
}

#[derive(Debug, Clone, Copy)]
struct PipelineWindow {
    buffer_mb: f64,
    buffer_usage_pct: f64,
    process_working_set_mb: Option<f64>,
    process_private_mb: Option<f64>,
}

#[derive(Debug, Clone, Copy)]
struct EncoderWindow {
    buffer_mb: f64,
    buffer_usage_pct: f64,
    pinned_snapshots_mb: f64,
    process_working_set_mb: Option<f64>,
    process_private_mb: Option<f64>,
}

/// Summarizes a set of scenario log runs into one suite summary.
pub fn summarize_benchmark_suite(
    inputs: &[BenchmarkRunInput],
    options: &BenchmarkSuiteOptions,
) -> Result<BenchmarkSuiteSummary, BenchmarkHarnessError> {
    if inputs.is_empty() {
        return Err(BenchmarkHarnessError::Input(
            "at least one scenario input is required".to_string(),
        ));
    }

    let mut scenarios = Vec::with_capacity(inputs.len());
    let mut quality_pass = 0usize;
    let mut missing_telemetry = 0usize;

    for input in inputs {
        let scenario_summary = summarize_single_scenario(input)?;
        if scenario_summary.quality.all_required_guardrails_passed {
            quality_pass = quality_pass.saturating_add(1);
        }
        if scenario_summary
            .quality
            .evaluations
            .iter()
            .any(|e| e.status == GuardrailStatus::MissingTelemetry)
        {
            missing_telemetry = missing_telemetry.saturating_add(1);
        }
        if options.strict_quality_guardrails
            && !scenario_summary.quality.all_required_guardrails_passed
        {
            return Err(BenchmarkHarnessError::MissingRequiredTelemetry {
                scenario: input.scenario,
                detail: format!(
                    "strict mode rejected scenario '{}': quality guardrails not fully satisfied",
                    input.scenario
                ),
            });
        }
        scenarios.push(scenario_summary);
    }

    scenarios.sort_by_key(|s| s.scenario.as_slug());

    let overall = OverallSummary {
        total_scenarios: scenarios.len(),
        scenarios_with_quality_pass: quality_pass,
        scenarios_with_missing_telemetry: missing_telemetry,
        strict_quality_guardrails: options.strict_quality_guardrails,
    };

    Ok(BenchmarkSuiteSummary {
        run_label: options.run_label.clone(),
        compare_to_label: options.compare_to_label.clone(),
        generated_by: "liteclip-core benchmark_harness v1",
        scenarios,
        overall,
    })
}

fn summarize_single_scenario(
    input: &BenchmarkRunInput,
) -> Result<ScenarioSummary, BenchmarkHarnessError> {
    let stdout_content = read_to_string(&input.stdout_log)?;
    let stderr_content = match &input.stderr_log {
        Some(path) => Some(read_to_string(path)?),
        None => None,
    };

    let parsed = parse_telemetry_from_logs(&stdout_content, stderr_content.as_deref());
    let metrics = build_metrics(&parsed);
    let quality = evaluate_quality(input.scenario, &parsed);
    let telemetry_windows = TelemetryWindowCounts {
        capture_windows: parsed.capture.len(),
        audio_windows: parsed.audio.len(),
        pipeline_windows: parsed.pipeline.len(),
        encoder_windows: parsed.encoder.len(),
    };
    let mut warnings = Vec::new();
    if telemetry_windows.capture_windows == 0
        && telemetry_windows.audio_windows == 0
        && telemetry_windows.pipeline_windows == 0
        && telemetry_windows.encoder_windows == 0
    {
        warnings.push(
            "No benchmark telemetry windows found. Ensure runtime logs include memory telemetry lines."
                .to_string(),
        );
    }

    Ok(ScenarioSummary {
        scenario: input.scenario,
        contract: *canonical_scenario_contract(input.scenario),
        run_label: input.label.clone(),
        stdout_log: input.stdout_log.clone(),
        stderr_log: input.stderr_log.clone(),
        telemetry_windows,
        metrics,
        quality,
        warnings,
    })
}

fn build_metrics(parsed: &ParsedTelemetry) -> ScenarioMetrics {
    let capture_fps = summarize_numbers(parsed.capture.iter().map(|w| w.fps));
    let capture_drop_ratio = summarize_numbers(parsed.capture.iter().map(|w| w.drop_ratio));
    let capture_divisor = summarize_numbers(parsed.capture.iter().map(|w| w.divisor));
    let capture_duplicates = summarize_numbers(parsed.capture.iter().map(|w| w.duplicates));
    let audio_pending_system = summarize_numbers(parsed.audio.iter().map(|w| w.pending_system));
    let audio_pending_mic = summarize_numbers(parsed.audio.iter().map(|w| w.pending_mic));
    let pipeline_buffer_mb = summarize_numbers(parsed.pipeline.iter().map(|w| w.buffer_mb));
    let pipeline_buffer_usage_pct =
        summarize_numbers(parsed.pipeline.iter().map(|w| w.buffer_usage_pct));
    let encoder_buffer_mb = summarize_numbers(parsed.encoder.iter().map(|w| w.buffer_mb));
    let encoder_buffer_usage_pct =
        summarize_numbers(parsed.encoder.iter().map(|w| w.buffer_usage_pct));
    let encoder_pinned_snapshots_mb =
        summarize_numbers(parsed.encoder.iter().map(|w| w.pinned_snapshots_mb));

    let working_set_from_capture = parsed
        .capture
        .iter()
        .filter_map(|w| w.process_working_set_mb);
    let working_set_from_audio = parsed.audio.iter().filter_map(|w| w.process_working_set_mb);
    let working_set_from_pipeline = parsed
        .pipeline
        .iter()
        .filter_map(|w| w.process_working_set_mb);
    let working_set_from_encoder = parsed
        .encoder
        .iter()
        .filter_map(|w| w.process_working_set_mb);
    let process_working_set_mb = summarize_numbers(
        working_set_from_capture
            .chain(working_set_from_audio)
            .chain(working_set_from_pipeline)
            .chain(working_set_from_encoder),
    );

    let private_from_capture = parsed.capture.iter().filter_map(|w| w.process_private_mb);
    let private_from_audio = parsed.audio.iter().filter_map(|w| w.process_private_mb);
    let private_from_pipeline = parsed.pipeline.iter().filter_map(|w| w.process_private_mb);
    let private_from_encoder = parsed.encoder.iter().filter_map(|w| w.process_private_mb);
    let process_private_mb = summarize_numbers(
        private_from_capture
            .chain(private_from_audio)
            .chain(private_from_pipeline)
            .chain(private_from_encoder),
    );

    ScenarioMetrics {
        capture_fps,
        capture_drop_ratio,
        capture_divisor,
        capture_duplicates,
        audio_pending_system,
        audio_pending_mic,
        pipeline_buffer_mb,
        pipeline_buffer_usage_pct,
        encoder_buffer_mb,
        encoder_buffer_usage_pct,
        encoder_pinned_snapshots_mb,
        process_working_set_mb,
        process_private_mb,
    }
}

fn evaluate_quality(
    scenario: CanonicalPerformanceScenario,
    parsed: &ParsedTelemetry,
) -> ScenarioQualitySummary {
    let contract = canonical_scenario_contract(scenario);
    let required_guardrails = contract.quality_guardrails.to_vec();
    let mut evaluations = Vec::with_capacity(required_guardrails.len());
    for guardrail in &required_guardrails {
        let evaluation = match guardrail {
            QualityGuardrailKind::DroppedFrames => evaluate_dropped_frames(parsed),
            QualityGuardrailKind::AvSync => GuardrailEvaluation {
                guardrail: QualityGuardrailKind::AvSync,
                status: GuardrailStatus::NotApplicable,
                detail:
                    "A/V sync telemetry is not currently emitted in benchmark logs; tracked externally."
                        .to_string(),
            },
            QualityGuardrailKind::ExportValidity => GuardrailEvaluation {
                guardrail: QualityGuardrailKind::ExportValidity,
                status: GuardrailStatus::NotApplicable,
                detail:
                    "Export validity is validated during export flow; benchmark logs currently have no direct export probe summary."
                        .to_string(),
            },
        };
        evaluations.push(evaluation);
    }

    let all_required_guardrails_passed = evaluations.iter().all(|e| {
        matches!(
            e.status,
            GuardrailStatus::Pass | GuardrailStatus::NotApplicable
        )
    });

    ScenarioQualitySummary {
        required_guardrails,
        evaluations,
        all_required_guardrails_passed,
    }
}

fn evaluate_dropped_frames(parsed: &ParsedTelemetry) -> GuardrailEvaluation {
    if parsed.capture.is_empty() {
        return GuardrailEvaluation {
            guardrail: QualityGuardrailKind::DroppedFrames,
            status: GuardrailStatus::MissingTelemetry,
            detail: "No capture telemetry windows available".to_string(),
        };
    }

    let mut failed_windows = 0usize;
    let mut max_drop_ratio = 0.0f64;
    let mut max_divisor = 0u32;
    for window in &parsed.capture {
        let divisor = window.divisor.max(0.0).round() as u32;
        max_drop_ratio = max_drop_ratio.max(window.drop_ratio.max(0.0));
        max_divisor = max_divisor.max(divisor);
        let exceeds_contract = window.drop_ratio
            > quality_contracts::ACTIVE_RECORDING_DROPPED_FRAMES_GUARDRAIL.max_drop_ratio
            || divisor
                > quality_contracts::ACTIVE_RECORDING_DROPPED_FRAMES_GUARDRAIL.max_fps_divisor;
        let explicit_fail = window.quality_contract_ok == Some(false);
        if exceeds_contract || explicit_fail {
            failed_windows = failed_windows.saturating_add(1);
        }
    }

    let limit = quality_contracts::ACTIVE_RECORDING_DROPPED_FRAMES_GUARDRAIL;
    if failed_windows == 0 {
        GuardrailEvaluation {
            guardrail: QualityGuardrailKind::DroppedFrames,
            status: GuardrailStatus::Pass,
            detail: format!(
                "All capture windows satisfied dropped-frame contract (max_drop_ratio={:.3}, max_fps_divisor={})",
                max_drop_ratio, max_divisor
            ),
        }
    } else {
        GuardrailEvaluation {
            guardrail: QualityGuardrailKind::DroppedFrames,
            status: GuardrailStatus::Fail,
            detail: format!(
                "{} capture window(s) exceeded dropped-frame contract (max_drop_ratio={:.3}, limit={:.3}; max_fps_divisor={}, limit={})",
                failed_windows, max_drop_ratio, limit.max_drop_ratio, max_divisor, limit.max_fps_divisor
            ),
        }
    }
}

/// Serializes a benchmark suite summary into stable JSON.
pub fn suite_summary_to_json(summary: &BenchmarkSuiteSummary) -> String {
    let mut out = String::with_capacity(32_768);
    out.push_str("{\n");
    write_json_field(&mut out, 1, "run_label", &summary.run_label);
    out.push_str(",\n");
    match &summary.compare_to_label {
        Some(label) => write_json_field(&mut out, 1, "compare_to_label", label),
        None => write_json_field_raw(&mut out, 1, "compare_to_label", "null"),
    }
    out.push_str(",\n");
    write_json_field(&mut out, 1, "generated_by", summary.generated_by);
    out.push_str(",\n");
    indent(&mut out, 1);
    out.push_str("\"overall\": ");
    write_overall_json(&mut out, &summary.overall);
    out.push_str(",\n");
    indent(&mut out, 1);
    out.push_str("\"scenarios\": [\n");
    for (idx, scenario) in summary.scenarios.iter().enumerate() {
        indent(&mut out, 2);
        write_scenario_json(&mut out, scenario);
        if idx + 1 != summary.scenarios.len() {
            out.push(',');
        }
        out.push('\n');
    }
    indent(&mut out, 1);
    out.push_str("]\n");
    out.push_str("}\n");
    out
}

fn write_overall_json(out: &mut String, overall: &OverallSummary) {
    out.push('{');
    write!(
        out,
        "\"total_scenarios\":{},\"scenarios_with_quality_pass\":{},\"scenarios_with_missing_telemetry\":{},\"strict_quality_guardrails\":{}",
        overall.total_scenarios,
        overall.scenarios_with_quality_pass,
        overall.scenarios_with_missing_telemetry,
        overall.strict_quality_guardrails
    )
    .ok();
    out.push('}');
}

fn write_scenario_json(out: &mut String, scenario: &ScenarioSummary) {
    out.push_str("{\n");
    write_json_field(out, 3, "scenario", scenario.scenario.as_slug());
    out.push_str(",\n");
    write_json_field(out, 3, "run_label", &scenario.run_label);
    out.push_str(",\n");
    write_json_field(out, 3, "stdout_log", &scenario.stdout_log.to_string_lossy());
    out.push_str(",\n");
    match &scenario.stderr_log {
        Some(path) => write_json_field(out, 3, "stderr_log", &path.to_string_lossy()),
        None => write_json_field_raw(out, 3, "stderr_log", "null"),
    }
    out.push_str(",\n");
    indent(out, 3);
    out.push_str("\"contract\": {");
    write!(
        out,
        "\"scope\":\"{}\",\"priority\":\"{}\",\"warmup_secs\":{},\"sample_secs\":{},\"quality_guardrails\":[",
        scenario.contract.scope.as_str(),
        scenario.contract.priority.as_str(),
        scenario.contract.warmup_secs,
        scenario.contract.sample_secs
    )
    .ok();
    for (idx, guardrail) in scenario.contract.quality_guardrails.iter().enumerate() {
        if idx > 0 {
            out.push(',');
        }
        out.push('"');
        out.push_str(guardrail.as_str());
        out.push('"');
    }
    out.push_str("],\"description\":");
    out.push_str(&json_escape(scenario.contract.description));
    out.push_str("},\n");

    indent(out, 3);
    out.push_str("\"telemetry_windows\": {");
    write!(
        out,
        "\"capture\":{},\"audio\":{},\"pipeline\":{},\"encoder\":{}",
        scenario.telemetry_windows.capture_windows,
        scenario.telemetry_windows.audio_windows,
        scenario.telemetry_windows.pipeline_windows,
        scenario.telemetry_windows.encoder_windows
    )
    .ok();
    out.push_str("},\n");

    indent(out, 3);
    out.push_str("\"metrics\": ");
    write_metrics_json(out, &scenario.metrics);
    out.push_str(",\n");

    indent(out, 3);
    out.push_str("\"quality\": ");
    write_quality_json(out, &scenario.quality);
    out.push_str(",\n");

    indent(out, 3);
    out.push_str("\"warnings\": [");
    for (idx, warning) in scenario.warnings.iter().enumerate() {
        if idx > 0 {
            out.push(',');
        }
        out.push_str(&json_escape(warning));
    }
    out.push_str("]\n");
    indent(out, 2);
    out.push('}');
}

fn write_metrics_json(out: &mut String, metrics: &ScenarioMetrics) {
    out.push('{');
    write_metric_field(out, "capture_fps", &metrics.capture_fps);
    out.push(',');
    write_metric_field(out, "capture_drop_ratio", &metrics.capture_drop_ratio);
    out.push(',');
    write_metric_field(out, "capture_divisor", &metrics.capture_divisor);
    out.push(',');
    write_metric_field(out, "capture_duplicates", &metrics.capture_duplicates);
    out.push(',');
    write_metric_field(out, "audio_pending_system", &metrics.audio_pending_system);
    out.push(',');
    write_metric_field(out, "audio_pending_mic", &metrics.audio_pending_mic);
    out.push(',');
    write_metric_field(out, "pipeline_buffer_mb", &metrics.pipeline_buffer_mb);
    out.push(',');
    write_metric_field(
        out,
        "pipeline_buffer_usage_pct",
        &metrics.pipeline_buffer_usage_pct,
    );
    out.push(',');
    write_metric_field(out, "encoder_buffer_mb", &metrics.encoder_buffer_mb);
    out.push(',');
    write_metric_field(
        out,
        "encoder_buffer_usage_pct",
        &metrics.encoder_buffer_usage_pct,
    );
    out.push(',');
    write_metric_field(
        out,
        "encoder_pinned_snapshots_mb",
        &metrics.encoder_pinned_snapshots_mb,
    );
    out.push(',');
    write_metric_field(
        out,
        "process_working_set_mb",
        &metrics.process_working_set_mb,
    );
    out.push(',');
    write_metric_field(out, "process_private_mb", &metrics.process_private_mb);
    out.push('}');
}

fn write_metric_field(out: &mut String, name: &str, metric: &NumericSummary) {
    out.push('"');
    out.push_str(name);
    out.push_str("\":");
    write_numeric_summary_json(out, metric);
}

fn write_numeric_summary_json(out: &mut String, summary: &NumericSummary) {
    out.push('{');
    write!(out, "\"samples\":{}", summary.samples).ok();
    out.push_str(",\"min\":");
    write_optional_number(out, summary.min);
    out.push_str(",\"max\":");
    write_optional_number(out, summary.max);
    out.push_str(",\"mean\":");
    write_optional_number(out, summary.mean);
    out.push_str(",\"p50\":");
    write_optional_number(out, summary.p50);
    out.push_str(",\"p95\":");
    write_optional_number(out, summary.p95);
    out.push('}');
}

fn write_quality_json(out: &mut String, quality: &ScenarioQualitySummary) {
    out.push('{');
    out.push_str("\"required_guardrails\":[");
    for (idx, guardrail) in quality.required_guardrails.iter().enumerate() {
        if idx > 0 {
            out.push(',');
        }
        out.push('"');
        out.push_str(guardrail.as_str());
        out.push('"');
    }
    out.push_str("],\"evaluations\":[");
    for (idx, evaluation) in quality.evaluations.iter().enumerate() {
        if idx > 0 {
            out.push(',');
        }
        out.push('{');
        write!(
            out,
            "\"guardrail\":\"{}\",\"status\":\"{}\",\"detail\":{}",
            evaluation.guardrail.as_str(),
            guardrail_status_str(evaluation.status),
            json_escape(&evaluation.detail)
        )
        .ok();
        out.push('}');
    }
    out.push_str("],\"all_required_guardrails_passed\":");
    out.push_str(if quality.all_required_guardrails_passed {
        "true"
    } else {
        "false"
    });
    out.push('}');
}

fn guardrail_status_str(status: GuardrailStatus) -> &'static str {
    match status {
        GuardrailStatus::Pass => "pass",
        GuardrailStatus::Fail => "fail",
        GuardrailStatus::MissingTelemetry => "missing-telemetry",
        GuardrailStatus::NotApplicable => "not-applicable",
    }
}

fn write_optional_number(out: &mut String, value: Option<f64>) {
    match value {
        Some(v) if v.is_finite() => {
            let rounded = (v * 10_000.0).round() / 10_000.0;
            write!(out, "{}", rounded).ok();
        }
        _ => out.push_str("null"),
    }
}

fn write_json_field(out: &mut String, indent_level: usize, key: &str, value: &str) {
    write_json_field_raw(out, indent_level, key, &json_escape(value));
}

fn write_json_field_raw(out: &mut String, indent_level: usize, key: &str, value: &str) {
    indent(out, indent_level);
    out.push('"');
    out.push_str(key);
    out.push_str("\": ");
    out.push_str(value);
}

fn indent(out: &mut String, level: usize) {
    for _ in 0..level {
        out.push_str("  ");
    }
}

fn json_escape(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 8);
    out.push('"');
    for ch in input.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                write!(&mut out, "\\u{:04x}", c as u32).ok();
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn summarize_numbers(values: impl IntoIterator<Item = f64>) -> NumericSummary {
    let mut samples: Vec<f64> = values.into_iter().filter(|v| v.is_finite()).collect();
    if samples.is_empty() {
        return NumericSummary::default();
    }

    samples.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
    let count = samples.len();
    let sum: f64 = samples.iter().sum();
    let min = samples.first().copied();
    let max = samples.last().copied();
    let mean = Some(sum / count as f64);
    let p50 = percentile(&samples, 0.50);
    let p95 = percentile(&samples, 0.95);
    NumericSummary {
        samples: count,
        min,
        max,
        mean,
        p50,
        p95,
    }
}

fn percentile(sorted: &[f64], quantile: f64) -> Option<f64> {
    if sorted.is_empty() {
        return None;
    }
    let q = quantile.clamp(0.0, 1.0);
    let idx = ((sorted.len() - 1) as f64 * q).round() as usize;
    sorted.get(idx).copied()
}

fn parse_telemetry_from_logs(stdout: &str, stderr: Option<&str>) -> ParsedTelemetry {
    let mut parsed = ParsedTelemetry::default();
    for line in stdout.lines() {
        parse_line(line, &mut parsed);
    }
    if let Some(stderr_content) = stderr {
        for line in stderr_content.lines() {
            parse_line(line, &mut parsed);
        }
    }
    parsed
}

fn parse_line(line: &str, parsed: &mut ParsedTelemetry) {
    let clean = strip_ansi(line);
    if clean.contains("Memory telemetry [capture]:") {
        if let Some(window) = parse_capture_window(clean.as_ref()) {
            parsed.capture.push(window);
        }
        return;
    }
    if clean.contains("Memory telemetry [audio]:") {
        if let Some(window) = parse_audio_window(clean.as_ref()) {
            parsed.audio.push(window);
        }
        return;
    }
    if clean.contains("Memory telemetry [pipeline]:") {
        if let Some(window) = parse_pipeline_window(clean.as_ref()) {
            parsed.pipeline.push(window);
        }
        return;
    }
    if clean.contains("Recording memory [encoder_periodic]:") {
        if let Some(window) = parse_encoder_window(clean.as_ref()) {
            parsed.encoder.push(window);
        }
    }
}

fn parse_capture_window(line: &str) -> Option<CaptureWindow> {
    let metrics = parse_key_value_metrics(line);
    Some(CaptureWindow {
        fps: metrics.get_number("fps")?,
        drop_ratio: metrics.get_number("drop_ratio")?,
        duplicates: metrics.get_number("duplicates").unwrap_or(0.0),
        divisor: metrics.get_number("divisor").unwrap_or(0.0),
        quality_contract_ok: metrics.get_bool("quality_contract_ok"),
        process_working_set_mb: metrics.get_number("process_working_set_mb"),
        process_private_mb: metrics.get_number("process_private_mb"),
    })
}

fn parse_audio_window(line: &str) -> Option<AudioWindow> {
    let metrics = parse_key_value_metrics(line);
    Some(AudioWindow {
        pending_system: metrics.get_number("pending_system")?,
        pending_mic: metrics.get_number("pending_mic")?,
        process_working_set_mb: metrics.get_number("process_working_set_mb"),
        process_private_mb: metrics.get_number("process_private_mb"),
    })
}

fn parse_pipeline_window(line: &str) -> Option<PipelineWindow> {
    let metrics = parse_key_value_metrics(line);
    Some(PipelineWindow {
        buffer_mb: metrics.get_number("buffer_mb")?,
        buffer_usage_pct: metrics.get_number("buffer_usage_pct")?,
        process_working_set_mb: metrics.get_number("process_working_set_mb"),
        process_private_mb: metrics.get_number("process_private_mb"),
    })
}

fn parse_encoder_window(line: &str) -> Option<EncoderWindow> {
    let metrics = parse_key_value_metrics(line);
    let buffer_mb = extract_number_after(line, "buffer=")?;
    let pinned_snapshots_mb = extract_number_after(line, "pinned_snapshots=")?;
    let buffer_usage_pct = extract_number_after(line, "mem=")?;
    Some(EncoderWindow {
        buffer_mb,
        buffer_usage_pct,
        pinned_snapshots_mb,
        process_working_set_mb: metrics.get_number("process_working"),
        process_private_mb: metrics.get_number("private"),
    })
}

#[derive(Debug, Clone, Default)]
struct MetricsMap(BTreeMap<String, String>);

impl MetricsMap {
    fn get_number(&self, key: &str) -> Option<f64> {
        self.0.get(key).and_then(|value| parse_number(value))
    }

    fn get_bool(&self, key: &str) -> Option<bool> {
        self.0.get(key).and_then(|value| {
            let normalized = value.trim().to_ascii_lowercase();
            match normalized.as_str() {
                "true" => Some(true),
                "false" => Some(false),
                _ => None,
            }
        })
    }
}

fn parse_key_value_metrics(line: &str) -> MetricsMap {
    let mut map = BTreeMap::<String, String>::new();
    let start = line.find(':').map(|idx| idx + 1).unwrap_or(0);
    let payload = &line[start..];
    for part in payload.split(',') {
        let trimmed = part.trim();
        if let Some((key, value)) = trimmed.split_once('=') {
            map.insert(key.trim().to_string(), value.trim().to_string());
        }
    }
    MetricsMap(map)
}

fn extract_number_after(line: &str, needle: &str) -> Option<f64> {
    let start = line.find(needle)? + needle.len();
    let suffix = &line[start..];
    let end = suffix
        .find(|c: char| !matches!(c, '0'..='9' | '.' | '-' | '+'))
        .unwrap_or(suffix.len());
    parse_number(&suffix[..end])
}

fn parse_number(input: &str) -> Option<f64> {
    let cleaned = input
        .trim()
        .trim_end_matches('%')
        .trim_end_matches("MB")
        .trim();
    cleaned.parse::<f64>().ok()
}

fn strip_ansi(line: &str) -> Cow<'_, str> {
    if !line.contains('\u{1b}') {
        return Cow::Borrowed(line);
    }
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' && chars.peek() == Some(&'[') {
            let _ = chars.next();
            for ctrl in chars.by_ref() {
                if ('@'..='~').contains(&ctrl) {
                    break;
                }
            }
            continue;
        }
        out.push(ch);
    }
    Cow::Owned(out)
}

fn read_to_string(path: &Path) -> Result<String, BenchmarkHarnessError> {
    std::fs::read_to_string(path).map_err(|source| BenchmarkHarnessError::ReadLog {
        path: path.to_path_buf(),
        source,
    })
}

/// Parses a scenario argument from CLI-like values.
pub fn parse_scenario_arg(
    value: &str,
) -> Result<CanonicalPerformanceScenario, BenchmarkHarnessError> {
    CanonicalPerformanceScenario::from_slug(value).ok_or_else(|| {
        BenchmarkHarnessError::Input(format!(
            "unknown scenario '{value}'. expected one of: idle-tray, active-recording, save-burst, gallery-playback-scrub, export"
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_artifacts_dir() -> PathBuf {
        let dir = std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join("target")
            .join("benchmark-harness-tests");
        std::fs::create_dir_all(&dir).expect("create benchmark harness test directory");
        dir
    }

    fn unique_path(name: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        test_artifacts_dir().join(format!("liteclip-bench-{name}-{stamp}.log"))
    }

    fn write_temp(content: &str, suffix: &str) -> PathBuf {
        let path = unique_path(suffix);
        std::fs::write(&path, content).expect("write temp log");
        path
    }

    #[test]
    fn parse_capture_pipeline_and_encoder_telemetry() {
        let stdout = r#"
INFO Memory telemetry [pipeline]: process_working_set_mb=101.2, process_private_mb=130.2, buffer_mb=9.1, buffer_packets=5402, buffer_usage_pct=1.8
INFO Memory telemetry [capture]: fps=57, drops=0, drop_ratio=0.000, duplicates=1213, divisor=0, quality_contract_ok=true, queue=0/32, nv12=Some((2, 0, 12)), bgra=Some((4, 0, 12)), process_working_set_mb=101.2, process_private_mb=130.2
INFO Recording memory [encoder_periodic]: process_working=101.2MB, private=130.2MB, buffer=9.1MB (5403pkts, 15kf, mem=2%), pinned_snapshots=0.0MB
"#;
        let parsed = parse_telemetry_from_logs(stdout, None);
        assert_eq!(parsed.capture.len(), 1);
        assert_eq!(parsed.pipeline.len(), 1);
        assert_eq!(parsed.encoder.len(), 1);
        assert_eq!(parsed.capture[0].fps, 57.0);
        assert_eq!(parsed.pipeline[0].buffer_mb, 9.1);
        assert_eq!(parsed.encoder[0].buffer_usage_pct, 2.0);
    }

    #[test]
    fn strips_ansi_from_telemetry_lines() {
        let line = "\u{1b}[32m INFO\u{1b}[0m Memory telemetry [capture]: fps=57, drops=0, drop_ratio=0.000, duplicates=1, divisor=0";
        let parsed = parse_telemetry_from_logs(line, None);
        assert_eq!(parsed.capture.len(), 1);
        assert_eq!(parsed.capture[0].fps, 57.0);
    }

    #[test]
    fn dropped_frame_guardrail_fails_when_limits_exceeded() {
        let stdout = r#"
INFO Memory telemetry [capture]: fps=30, drops=25, drop_ratio=0.455, duplicates=0, divisor=3, quality_contract_ok=false, queue=16/32, nv12=None, bgra=None
"#;
        let parsed = parse_telemetry_from_logs(stdout, None);
        let quality = evaluate_quality(CanonicalPerformanceScenario::ActiveRecording, &parsed);
        let dropped = quality
            .evaluations
            .iter()
            .find(|e| e.guardrail == QualityGuardrailKind::DroppedFrames)
            .expect("dropped-frame evaluation");
        assert_eq!(dropped.status, GuardrailStatus::Fail);
        assert!(!quality.all_required_guardrails_passed);
    }

    #[test]
    fn summarize_suite_from_baseline_like_logs() {
        let active_stdout = r#"
INFO Memory telemetry [pipeline]: process_working_set_mb=101.2, process_private_mb=130.2, buffer_mb=9.1, buffer_packets=5402, buffer_usage_pct=1.8
INFO Memory telemetry [capture]: fps=57, drops=0, drop_ratio=0.000, duplicates=1213, divisor=0, quality_contract_ok=true, queue=0/32, nv12=Some((2, 0, 12)), bgra=Some((4, 0, 12)), process_working_set_mb=101.2, process_private_mb=130.2
INFO Recording memory [encoder_periodic]: process_working=101.2MB, private=130.2MB, buffer=9.1MB (5403pkts, 15kf, mem=2%), pinned_snapshots=0.0MB
"#;
        let idle_stdout = r#"
INFO LiteClip started
"#;
        let active_path = write_temp(active_stdout, "active");
        let idle_path = write_temp(idle_stdout, "idle");
        let inputs = vec![
            BenchmarkRunInput {
                label: "active".to_string(),
                scenario: CanonicalPerformanceScenario::ActiveRecording,
                stdout_log: active_path.clone(),
                stderr_log: None,
            },
            BenchmarkRunInput {
                label: "idle".to_string(),
                scenario: CanonicalPerformanceScenario::IdleTray,
                stdout_log: idle_path.clone(),
                stderr_log: None,
            },
        ];
        let suite =
            summarize_benchmark_suite(&inputs, &BenchmarkSuiteOptions::single_run("baseline"))
                .expect("suite summary");
        assert_eq!(suite.overall.total_scenarios, 2);
        let active = suite
            .scenarios
            .iter()
            .find(|s| s.scenario == CanonicalPerformanceScenario::ActiveRecording)
            .expect("active summary");
        assert_eq!(active.telemetry_windows.capture_windows, 1);
        assert_eq!(active.metrics.capture_fps.mean, Some(57.0));
        let json = suite_summary_to_json(&suite);
        assert!(json.contains("\"run_label\": \"baseline\""));
        assert!(json.contains("\"scenario\": \"active-recording\""));

        let _ = std::fs::remove_file(active_path);
        let _ = std::fs::remove_file(idle_path);
    }

    #[test]
    fn scenario_arg_parser_accepts_aliases() {
        assert_eq!(
            parse_scenario_arg("active_recording").ok(),
            Some(CanonicalPerformanceScenario::ActiveRecording)
        );
        assert_eq!(
            parse_scenario_arg("gallery").ok(),
            Some(CanonicalPerformanceScenario::GalleryPlaybackScrub)
        );
        assert!(parse_scenario_arg("unknown").is_err());
    }
}
