# Plan: Automatic Memory Limit Recommendation

## Status
Pending

## Priority
Low

## Summary
Add dynamic memory limit recommendations based on actual observed memory usage during recording. Currently the settings UI shows a static "recommended" value. A live recommendation based on measured bytes-per-second would help users optimize their memory budget.

## Current State
- `memory_limit_mb` config has an auto mode
- Settings UI shows a static recommended value
- No dynamic recommendation based on actual usage
- Users may not know how much memory their settings consume

## Implementation Steps

### 1. Bytes-Per-Second Tracking
- Track encoded bytes per second during recording
- Calculate running average over the last 60 seconds
- Track peak usage and average usage separately
- Store statistics in `AppState`

### 2. Recommendation Engine
- Calculate recommended memory for the configured replay duration:
  - `recommended = bytes_per_second * replay_duration_secs * 1.2` (20% headroom)
- Factor in concurrent snapshot overhead (512MB max)
- Provide minimum, recommended, and comfortable values

### 3. GUI Display
- Show dynamic recommendation in the Advanced settings tab:
  - "Based on current settings: ~X MB for Y second clips"
  - "Observed usage: Z MB/min average, W MB/min peak"
  - Color-coded: green (plenty), yellow (tight), red (insufficient)
- Update recommendation live as settings change

### 4. Auto-Tune Option
- Optional: automatically adjust memory limit based on observed usage
- Never reduce below minimum safe value
- Notify user when auto-tuning changes the limit

### 5. Historical Data
- Store usage statistics across sessions
- Show trend: "Memory usage has increased 15% since last week"
- Suggest settings changes if memory is consistently tight

## Files to Modify
- `crates/liteclip-core/src/config/config_mod/types.rs` — Observed bytes/sec tracking
- `crates/liteclip-core/src/app/state.rs` — Memory usage statistics
- `crates/liteclip-core/src/buffer/ring/spmc_ring.rs` — Report memory usage metrics
- `src/gui/settings.rs` — Dynamic recommendation display

## Estimated Effort
Small (1-2 days)

## Dependencies
- None

## Risks
- Usage patterns vary significantly between content types
- Recommendation may be inaccurate for short sessions
- Auto-tuning must not cause instability
