# Plan: Benchmark Scenario Expansion

## Status
Pending

## Priority
Low

## Summary
Expand the benchmark harness with additional scenarios to improve regression prevention. Currently 5 canonical scenarios exist (idle-tray, active-recording, save-burst, gallery-playback-scrub, export) but key usage patterns are not covered.

## Current State
- 5 benchmark scenarios: IdleTray, ActiveRecording, SaveBurst, GalleryPlaybackScrub, Export
- Quality contracts define performance guardrails
- Missing scenarios for: multi-clip concurrent save, high-FPS capture, multi-monitor, audio-only
- Expanding coverage ensures performance guarantees across the full feature set

## Implementation Steps

### 1. New Scenarios
- **MultiClipConcurrentSave**: Save 3+ clips simultaneously, measure memory and time
- **HighFPSCapture**: Record at 120+ FPS, measure encoder latency and frame drops
- **MultiMonitorCapture**: Capture from multiple monitors (when multi-monitor support exists)
- **AudioOnlyCapture**: Record with video disabled, measure audio pipeline performance
- **LongSessionRecording**: Simulate a 30-minute recording session, measure memory stability
- **RapidHotkeySpam**: Rapidly toggle save clip hotkey, measure buffer resilience
- **LowBitrateQuality**: Record at low bitrate, measure quality metrics (PSNR/SSIM)

### 2. Quality Contracts
- Define performance thresholds for each new scenario
- Add quality metrics (PSNR, SSIM) for video quality regression detection
- Add audio quality metrics (THD+N, frequency response) for audio regression

### 3. Telemetry Enhancements
- Add GPU telemetry to benchmark results
- Add disk I/O metrics (read/write throughput, latency)
- Add network metrics (if webhook integration exists)

### 4. CI Integration
- Run benchmark suite on every PR (hardware permitting)
- Compare results against baseline
- Block PRs that regress beyond threshold
- Store benchmark results as GitHub artifacts

### 5. Reporting
- Generate HTML report with charts
- Compare current run against previous runs
- Highlight regressions and improvements
- Export results as JSON for external analysis

## Files to Modify
- `crates/liteclip-core/src/quality_contracts.rs` — New scenario variants and contracts
- `crates/liteclip-core/src/benchmark_harness.rs` — New scenario parsing and telemetry
- `crates/liteclip-core/src/memory_diag.rs` — Extended telemetry for benchmarks
- `src/bin/` — Benchmark CLI tool enhancements

## Estimated Effort
Medium (3-5 days)

## Dependencies
- Existing benchmark harness infrastructure

## Risks
- Some scenarios require specific hardware (multi-monitor, high-FPS display)
- Benchmark results may vary between runs (noise)
- CI environment may not have GPU access for hardware encoding tests
