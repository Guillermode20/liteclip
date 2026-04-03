# Plan: Filter-Graph Export Pipeline

## Status
Pending

## Priority
Medium

## Summary
Implement a filter-graph based export pipeline for the clip editor. Currently the export uses seek-based range processing. A filter-graph approach would enable more sophisticated operations like precise trimming, overlays, text annotations, and complex multi-segment cuts without re-encoding the entire timeline.

## Current State
- `video_file.rs:547` notes a prototype flag for filter-graph based export is "planned but not yet wired"
- Current export uses seek-based range processing with stream copy when possible
- Multi-segment cut points are supported but processed via repeated seek+copy operations
- No filter graph infrastructure exists

## Implementation Steps

### 1. FFmpeg Filter Graph Abstraction
- Create a `FilterGraph` builder type in the output module
- Support common filters: `trim`, `setpts`, `concat`, `scale`, `overlay`, `drawtext`
- Generate FFmpeg filter complex strings from the builder

### 2. Precise Trim Filter
- Replace seek-based trimming with `trim=start:end` + `setpts` filter chain
- Frame-accurate cuts instead of keyframe-dependent seeks
- Support multiple trim segments with `concat` filter

### 3. Integration with Clip Editor
- Wire up the existing multi-segment cut UI to the filter graph builder
- Generate the filter complex string from enabled/disabled timeline segments
- Maintain stream copy mode when no filters are needed (no re-encode)

### 4. Future Extensibility
- Design the filter graph builder to support future features:
  - Text overlays (timestamps, watermarks)
  - Picture-in-picture
  - Speed adjustment (slow motion / fast forward)
  - Audio filters (fade in/out, volume automation)

### 5. Performance
- Use hardware-accelerated filters where possible (scale_cuda, overlay_cuda)
- Fall back to CPU filters when hardware not available
- Progress reporting for long exports

## Files to Modify
- `crates/liteclip-core/src/output/video_file.rs` — Add filter-graph export path
- `crates/liteclip-core/src/output/` — New `filter_graph.rs` module
- `crates/liteclip-core/src/encode/ffmpeg/` — Hardware filter support
- `src/gui/gallery/editor.rs` — Wire filter graph to export button
- `src/gui/gallery/types.rs` — Extend export options with filter settings

## Estimated Effort
Large (5-7 days)

## Dependencies
- FFmpeg with filter support (standard builds include this)
- Clip editor multi-segment UI (already implemented)

## Risks
- Filter graphs are complex and error-prone; thorough testing needed
- Hardware filter availability varies by GPU and FFmpeg build
- Concat filter requires matching codec parameters across segments
