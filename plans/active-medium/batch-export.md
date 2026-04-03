# Plan: Batch Export from Gallery

## Status
Pending

## Priority
Medium

## Summary
Add batch export capability to the gallery. Currently the editor only exports one clip at a time. Users should be able to select multiple clips and apply the same export settings to all of them.

## Current State
- Gallery editor exports one clip at a time
- Multi-select mode exists for bulk deletion
- No batch export state machine or progress tracking
- Users with many clips must export each one individually

## Implementation Steps

### 1. Batch Export State Machine
- Extend multi-select mode to support batch export action
- Collect selected clips and export settings (target size, encoder, resolution)
- Process clips sequentially or in parallel (configurable concurrency)
- Track per-clip progress and overall batch progress

### 2. Export Queue
- Create an export queue with priority ordering
- Support pause/resume of batch export
- Show queue status: "Exporting 3 of 12 clips"
- Allow adding more clips to an in-progress batch

### 3. Progress UI
- New batch export panel in the gallery
- Show per-clip progress bars with status (queued, exporting, done, failed)
- Overall progress bar with estimated time remaining
- Cancel button to stop remaining exports

### 4. Output Organization
- Batch export to a dedicated subdirectory (e.g., `exports/batch_YYYY-MM-DD/`)
- Use custom filename templates for batch exports
- Generate a summary file listing all exported clips and their settings

### 5. Error Handling
- Continue batch export if individual clip fails
- Log failures and show summary at completion
- Allow retrying failed clips

### 6. Performance
- Use `rayon` for parallel encoding when clips are independent
- Limit concurrency to avoid overwhelming the encoder/GPU
- Respect memory limits across concurrent exports

## Files to Modify
- `src/gui/gallery.rs` — Batch export state machine, progress tracking
- `src/gui/gallery/browser.rs` — Batch export UI in selection panel
- `src/gui/gallery/editor.rs` — Batch export settings panel
- `crates/liteclip-core/src/output/video_file.rs` — Batch export API
- `crates/liteclip-core/src/app/clip.rs` — Export queue management

## Estimated Effort
Medium (3-5 days)

## Dependencies
- Multi-select mode (already implemented)
- Export pipeline (already implemented)

## Risks
- Parallel exports may compete for GPU/CPU resources
- Memory usage scales with concurrent exports
- Long batch exports may be interrupted by system sleep
