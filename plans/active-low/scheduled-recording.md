# Plan: Scheduled and Time-Limited Recording

## Status
Pending

## Priority
Low

## Summary
Add the ability to start recording at a specific time or stop after a fixed duration. This bridges the gap between a replay recorder and a traditional screen recorder, enabling scheduled captures for streams or events.

## Current State
- No scheduling capability exists
- Recording is entirely manual (hotkey or tray toggle)
- No timer or countdown mechanism exists
- Replay buffer always runs when enabled

## Implementation Steps

### 1. Schedule Configuration
- Add `RecordingSchedule` struct to config:
  - `start_time: Option<NaiveTime>` — Start recording at this time daily
  - `start_datetime: Option<NaiveDateTime>` — One-time scheduled start
  - `duration_secs: Option<u64>` — Auto-stop after this duration
  - `enabled: bool`

### 2. Scheduler Engine
- Background timer that checks schedule conditions
- Trigger recording start when conditions are met
- Trigger recording stop when duration elapses
- Support both recurring (daily) and one-time schedules

### 3. Integration with Recording Pipeline
- Schedule triggers the same recording start/stop as manual hotkeys
- Replay buffer continues running; schedule controls when clips are saved
- Or: schedule controls when the replay buffer is active

### 4. GUI
- Add schedule configuration tab to settings
- Time picker for start time
- Duration selector (minutes/hours)
- Recurring vs. one-time toggle
- Show next scheduled action in status

### 5. Notifications
- Toast notification when scheduled recording starts
- Toast notification when scheduled recording stops
- Notification if schedule conflicts with existing recording

## Files to Modify
- `crates/liteclip-core/src/config/config_mod/types.rs` — Add schedule config struct
- `crates/liteclip-core/src/app/state.rs` — Scheduler state management
- `crates/liteclip-core/src/engine.rs` — Schedule API for embedders
- `src/main.rs` — Timer integration in event loop
- `src/gui/settings.rs` — Schedule configuration UI tab

## Estimated Effort
Medium (3-5 days)

## Dependencies
- None

## Risks
- System sleep/hibernate may interfere with scheduled starts
- Time zone changes and DST transitions need handling
- Scheduler must survive application restarts
