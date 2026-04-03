# Plan: GPU Usage Telemetry & Performance Dashboard

## Status
Pending

## Priority
Low

## Summary
Add a performance dashboard showing real-time GPU usage, encoder statistics, memory consumption, and frame timing. This helps users diagnose performance issues and optimize their recording settings.

## Current State
- Memory telemetry logging exists (`memory_diag.rs`) -- logs every 30s
- Memory limit configuration exists (auto or manual, 256MB-8192MB)
- Encoder runs at BELOW_NORMAL priority
- No real-time performance metrics are exposed to the user
- Benchmark harness exists for internal testing

## Implementation Steps

### 1. Metrics Collection
- **GPU**: Utilization %, VRAM usage, GPU temperature (via DXGI/DirectML or NVAPI/ADL)
- **Encoder**: Frames per second, encoding latency, bitrate, dropped frames
- **Capture**: Frame acquisition time, frame drops, DXGI latency
- **Memory**: Ring buffer usage, total process memory, eviction rate
- **Audio**: Buffer underruns, latency, RNNoise processing time

### 2. Real-Time Dashboard
- Add a "Performance" tab to the settings window (or a floating overlay)
- Show live graphs for key metrics (GPU, memory, FPS)
- Color-code warnings (e.g., red when memory > 80%, yellow when encoding latency > frame time)
- Toggle between summary view and detailed view

### 3. Historical Data
- Store metrics in a ring buffer (last 5 minutes)
- Allow zooming into the time range
- Export metrics to CSV for analysis

### 4. OSD Overlay (Optional)
- Optional on-screen display while gaming
- Show FPS, encoding latency, and memory usage
- Toggle via hotkey
- Minimal performance impact

### 5. Alerts
- Notify user when:
  - Encoder falls behind (dropped frames)
  - Memory usage exceeds threshold
  - GPU utilization is too low (bottleneck elsewhere)
  - Audio buffer underruns detected

## Files to Modify
- `crates/liteclip-core/src/` -- New `metrics/` module
- `crates/liteclip-core/src/memory_diag.rs` -- Extend with more metrics
- `crates/liteclip-core/src/encode/` -- Add encoder statistics
- `crates/liteclip-core/src/capture/` -- Add capture statistics
- `src/gui/settings.rs` -- Add performance dashboard tab
- `src/gui/` -- New `performance/` module with charts

## Estimated Effort
Medium-Large (4-6 days)

## Dependencies
- GPU vendor SDKs for detailed metrics (optional, NVAPI for NVIDIA, ADL for AMD)
- Chart rendering library or custom egui chart component

## Risks
- Metric collection must not impact recording performance
- GPU vendor SDKs add platform-specific dependencies
- OSD overlay requires careful integration to avoid capture interference
