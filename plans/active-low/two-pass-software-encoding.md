# Plan: Two-Pass Software Encoding

## Status
Pending

## Priority
Low

## Summary
Implement two-pass encoding for the software (CPU) export path. Two-pass encoding analyzes the video in a first pass to determine optimal bit allocation, then encodes in a second pass for better quality at the same bitrate. This is particularly valuable for clip exports where file size matters.

## Current State
- `video_file.rs:727` notes a "software two-pass prototype flag is enabled" but "two-pass implementation is not yet wired"
- Single-pass software encoding uses `libx265` (HEVC) or would use `libx264` (H.264)
- Target file size control exists but is less accurate without two-pass analysis

## Implementation Steps

### 1. Two-Pass libx265/libx264 Support
- Configure FFmpeg encoder with `-pass 1` and `-pass 2` flags
- First pass: analysis only, output to `/dev/null` (or temp file on Windows)
- Second pass: actual encoding using the log file from pass 1
- Use `libx265` and `libx264` with `-x265-params pass=1/2` or `-x264-params pass=1/2`

### 2. Export Pipeline Integration
- Detect when two-pass is beneficial (target file size set, software encoder selected)
- Run pass 1, collect stats, then run pass 2
- Show progress for both passes in the GUI (Pass 1/2: XX%)
- Clean up temporary log files after completion

### 3. Configuration
- Add `two_pass_encoding: bool` to export settings (default: true for software)
- Allow users to disable for faster (but lower quality) exports
- Auto-enable when target file size is manually specified

### 4. GUI Updates
- Update export progress UI to show two-pass progress
- Indicate which pass is running
- Estimated time remaining should account for both passes

## Files to Modify
- `crates/liteclip-core/src/encode/ffmpeg/software.rs` — Add two-pass encoding support
- `crates/liteclip-core/src/output/video_file.rs` — Wire two-pass into export pipeline
- `crates/liteclip-core/src/config/config_mod/types.rs` — Add two-pass config option
- `src/gui/gallery/editor.rs` — Update progress reporting for two-pass

## Estimated Effort
Medium (2-3 days)

## Dependencies
- Software encoder (libx265/libx264) must be available
- Target file size feature (already implemented)

## Risks
- Two-pass doubles the encoding time
- Temporary log files must be cleaned up even on failure
- Not all FFmpeg builds support two-pass for all codecs
