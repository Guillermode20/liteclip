# User Testing: GUI Thread CPU Reduction

## Validation Surface

**Application Type:** Native Windows desktop application (egui/winit)

**Surface:** GUI thread behavior when idle - CPU usage, timing, responsiveness

**Testing Constraint:** No automated browser testing available for native Windows applications. All validation is manual user testing.

## Required Testing Skills/Tools

| Tool | Purpose | Notes |
|------|---------|-------|
| Windows Task Manager | CPU/memory measurement | Details tab, filter by process name |
| Video recording (OBS/Game Bar) | Timing analysis | Frame-by-frame toast/window appearance |
| Stopwatch/Timer | Latency measurement | Human reaction time ~200ms (account for this) |
| FFprobe | Clip validation | Verify saved clips are valid MP4 |
| Debug logs | Wake timing analysis | `RUST_LOG=debug` for event loop state |

## Resource Cost Classification

**Max Concurrent Validators:** 1

**Rationale:**
- This is a single-user desktop application
- Manual testing requires user interaction
- Cannot parallelize manual observation
- CPU measurement requires consistent system state (no other load)

## Test Execution Protocol

### Prerequisites

1. **Build release**: `cargo build --release --features ffmpeg`
2. **Close other applications**: Minimize background CPU usage
3. **Open Task Manager**: Details tab, sort by CPU
4. **Enable debug logging**: `$env:RUST_LOG="debug,liteclip_core=trace"`

### Baseline Measurement (Before Changes)

1. Run application for 10 seconds idle
2. Record CPU usage average
3. Record memory usage
4. Test toast timing (save clip)
5. Test settings/gallery opening timing

### Post-Change Measurement

Repeat all baseline measurements after changes.

### Evidence Collection

- Screenshots of Task Manager CPU/memory columns
- Video recordings of toast/window appearance timing
- Debug logs showing dormancy transitions
- FFprobe output for saved clips

## Timing Thresholds

| Assertion | Threshold | Measurement Method |
|-----------|-----------|-------------------|
| CPU idle | <0.1% | Task Manager 10s average |
| Toast appearance | <100ms | Video frame count |
| Settings opening | <200ms | Stopwatch/video |
| Gallery opening | <200ms | Stopwatch/video |
| Dormancy activation | <3s | Task Manager observation |
| Hotkey response | <100ms | Stopwatch/video |

## Accepted Limitations

- **Manual timing**: Human reaction time (~200ms) affects stopwatch measurements
- **Video analysis**: Frame count depends on recording frame rate
- **CPU measurement**: System load variations affect Task Manager readings
- **No automated GUI testing**: Native Windows app limitation
