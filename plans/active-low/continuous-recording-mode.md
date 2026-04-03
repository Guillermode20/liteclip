# Plan: Continuous Recording Mode

## Status
Pending

## Priority
Low

## Summary
Add a continuous direct-to-disk recording mode for long sessions. LiteClip is exclusively a replay buffer recorder. A continuous mode that writes directly to disk (bypassing the ring buffer) complements the existing replay functionality for streaming or monitoring use cases.

## Current State
- Only replay buffer mode exists
- No direct-to-disk recording capability
- All frames go through the ring buffer in RAM
- Long sessions require large memory budgets

## Implementation Steps

### 1. Recording Mode Enum
- Add `RecordingMode` to config: `Replay | Continuous`
- **Replay**: current behavior (ring buffer, save last N seconds)
- **Continuous**: direct-to-disk recording, split into segments

### 2. Direct-to-Disk Pipeline
- Bypass the ring buffer in continuous mode
- Feed encoded packets directly to the MP4 muxer
- Write to disk in real-time
- Segment files by duration or size (configurable)

### 3. Segment Configuration
- Add `ContinuousConfig`:
  - `segment_duration_secs: u64` — Split files every N seconds (default: 300 = 5 min)
  - `segment_size_mb: u64` — Split files at N MB (alternative to duration)
  - `max_segments: u32` — Keep only the last N segments (circular)
  - `auto_delete_old: bool` — Delete segments beyond max_segments

### 4. File Naming
- Continuous mode filenames include segment number:
  - `YYYY-MM-DD_HH-MM-SS_segment_001.mp4`
- Maintain game-based folder organization

### 5. GUI
- Add recording mode toggle to the General tab
- Show segment status in tray tooltip
- Gallery shows continuous segments with appropriate icons

### 6. Resource Management
- Continuous mode uses less memory (no ring buffer)
- Disk I/O becomes the bottleneck — monitor and warn
- Segment rotation prevents unbounded disk usage

## Files to Modify
- `crates/liteclip-core/src/app/pipeline/manager.rs` — Continuous recording mode
- `crates/liteclip-core/src/output/video_file.rs` — Direct-to-disk muxer
- `crates/liteclip-core/src/config/config_mod/types.rs` — Recording mode enum and config
- `crates/liteclip-core/src/engine.rs` — Continuous mode API
- `src/main.rs` — Mode selection in startup
- `src/gui/settings.rs` — Mode toggle in General tab

## Estimated Effort
Large (5-8 days)

## Dependencies
- None

## Risks
- Disk I/O bottleneck during continuous recording
- Segment boundaries may cut mid-action
- File system fragmentation from many small files
- Must handle disk full gracefully (stop recording, notify user)
