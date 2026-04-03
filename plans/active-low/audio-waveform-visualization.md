# Plan: Audio Waveform Visualization

## Status
Pending

## Priority
Low

## Summary
Add audio waveform visualization to the clip editor timeline and gallery browser. This provides visual feedback for audio content, making it easier to identify interesting moments, silence gaps, and audio quality issues.

## Current State
- Clip editor has a timeline scrubber with video preview
- No audio waveform is displayed on the timeline
- Audio level meters exist in the settings window (live monitoring)
- FFmpeg can extract audio data for waveform generation

## Implementation Steps

### 1. Waveform Generation
- Extract audio samples from clip files using FFmpeg
- Compute RMS amplitude per time bucket (e.g., 100 buckets per second)
- Generate waveform data as a vector of amplitude values
- Cache waveform data alongside clip thumbnails

### 2. Timeline Integration
- Render waveform below the video timeline in the clip editor
- Support zoom in/out (waveform resolution adjusts)
- Show separate waveforms for system audio and microphone (stacked or overlaid)
- Color-code waveforms per source (e.g., blue for system, green for mic)

### 3. Gallery Browser
- Show mini waveform on clip cards in the gallery
- Helps identify clips with/without audio at a glance
- Keep it subtle -- don't overwhelm the thumbnail

### 4. Performance
- Generate waveforms asynchronously in a background thread
- Use `rayon` for parallel processing of multiple clips
- Cache waveform data to disk (invalidate when clip is modified)
- Progressive loading: show placeholder, then low-res, then high-res

### 5. Export Integration
- When exporting a clip segment, regenerate waveform for the exported portion
- Update waveform display after export completes

## Files to Modify
- `crates/liteclip-core/src/output/` -- Add waveform generation module
- `crates/liteclip-core/src/output/companion_cache.rs` -- Cache waveform data
- `src/gui/gallery/editor.rs` -- Render waveform on timeline
- `src/gui/gallery/browser.rs` -- Show mini waveform on clip cards
- `src/gui/gallery/types.rs` -- Add waveform data types

## Estimated Effort
Medium (3-4 days)

## Dependencies
- FFmpeg audio decode (already available)
- `egui` drawing primitives for waveform rendering

## Risks
- Waveform generation adds processing time for large clip libraries
- Cache invalidation must handle clip edits and exports
- Memory usage for waveform data across many clips
