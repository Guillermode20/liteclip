# Plan: Pause/Resume Recording

## Status
Pending

## Priority
High

## Summary
Add pause/resume capability to the recording pipeline. Currently the recording can only be toggled on/off entirely. Pausing keeps the buffer alive but stops accumulating content, allowing users to skip loading screens, cutscenes, or breaks without losing their replay buffer.

## Current State
- Recording pipeline can only be toggled on/off entirely
- Stopping recording clears the entire replay buffer
- No pause state exists in the pipeline state machine
- Users must stop and restart, losing all buffered content

## Implementation Steps

### 1. Pause State Machine
- Add `Paused` state to the pipeline lifecycle: `Idle -> Recording -> Paused -> Recording -> ...`
- Pause stops feeding frames to the encoder and ring buffer
- Encoder remains initialized (no warmup needed on resume)
- Ring buffer retains its content during pause

### 2. Video Pipeline
- Add `pause()` and `resume()` methods to the video pipeline
- On pause: stop frame acquisition, flush any in-flight encode
- On resume: reacquire frames and continue encoding
- Track pause duration for accurate timestamps

### 3. Audio Pipeline
- Pause audio capture (system and mic)
- On resume, audio streams may need reinitialization
- Handle audio gap gracefully (silence or skip)

### 4. Hotkey and UI
- Add `pause_recording` hotkey to `HotkeyConfig`
- Add "Pause Recording" / "Resume Recording" to tray menu
- Update tray icon or tooltip to reflect paused state
- Recording indicator should show "PAUSED" state

### 5. Clip Save During Pause
- Saving a clip during pause should capture from before the pause
- Clip timestamps should not include the pause duration
- Gallery should indicate if a clip spans a pause boundary

## Files to Modify
- `crates/liteclip-core/src/app/pipeline/manager.rs` — Pause/resume state machine
- `crates/liteclip-core/src/app/pipeline/video.rs` — Pause flag in encode loop
- `crates/liteclip-core/src/app/pipeline/audio.rs` — Pause flag in audio capture
- `crates/liteclip-core/src/buffer/ring/spmc_ring.rs` — Pause-aware snapshot logic
- `crates/liteclip-core/src/config/config_mod/types.rs` — Add pause hotkey config
- `src/platform/hotkeys.rs` — Add pause hotkey
- `src/platform/tray.rs` — Add pause menu item
- `src/gui/settings.rs` — Pause hotkey field
- `src/main.rs` — Wire pause hotkey handler

## Estimated Effort
Large (5-8 days)

## Dependencies
- None

## Risks
- Audio gap handling is complex (silence insertion vs. timestamp skip)
- Encoder state must be preserved during pause (no reinit on resume)
- Timestamp continuity across pause/resume boundaries
- Ring buffer age calculations during pause period
