# Plan: Crash Recovery and Corrupted Clip Detection

## Status
Pending

## Priority
High

## Summary
Implement crash recovery for partially written clip files. If the application crashes during clip save, the broken MP4 file is left on disk with no cleanup. A recovery mechanism that detects, repairs, or removes corrupted files improves reliability.

## Current State
- Clip save writes directly to the output file
- Crashes during save leave partially written, unplayable MP4 files
- No mechanism to detect or clean up corrupted files
- Users may not realize a clip save failed until they try to play it

## Implementation Steps

### 1. Atomic Write via Temp File
- Write clip to a temporary file first (e.g., `clip.mp4.tmp`)
- On successful muxer close, rename temp file to final path
- OS-level rename is atomic — no partial files on disk
- Clean up orphaned `.tmp` files on startup

### 2. Corrupted File Detection
- On startup, scan the save directory for:
  - Orphaned `.tmp` files (incomplete saves)
  - MP4 files with invalid moov atoms (corrupted)
- Flag corrupted files in the gallery with a warning icon
- Offer to delete or attempt repair

### 3. MP4 Repair Attempt
- Use FFmpeg to attempt recovery of corrupted MP4 files
- Extract any usable video/audio streams
- Save recovered version with `_recovered` suffix
- Log recovery success/failure

### 4. Save Integrity Verification
- After clip save completes, verify the output file:
  - File size > 0
  - Valid MP4 header (ftyp atom present)
  - At least one video stream
- If verification fails, retry the save or log an error

### 5. GUI Integration
- Show corrupted file warning in gallery browser
- Context menu option: "Delete corrupted file" or "Attempt repair"
- Startup notification if corrupted files were found and cleaned

## Files to Modify
- `crates/liteclip-core/src/output/saver.rs` — Atomic write via temp file + rename
- `crates/liteclip-core/src/output/functions.rs` — Corrupted file detection on startup
- `crates/liteclip-core/src/output/video_file.rs` — MP4 integrity verification
- `src/main.rs` — Startup recovery scan
- `src/gui/gallery/browser.rs` — Detect and flag corrupted clips

## Estimated Effort
Medium (3-5 days)

## Dependencies
- None

## Risks
- Atomic rename may fail across filesystem boundaries (temp dir vs. save dir)
- MP4 repair is not guaranteed to succeed
- Verification adds latency to the save pipeline
