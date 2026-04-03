# Plan: Separate Audio Tracks in Export

## Status
Pending

## Priority
Low

## Summary
Allow exporting clips with separate audio tracks (system audio on track 1, mic on track 2) for post-production editing. Currently the audio mixer combines system and mic into a single track, which is a standard feature in professional tools like OBS.

## Current State
- Audio mixer combines system and mic into a single stereo track
- No multi-track recording or export capability
- Content creators cannot adjust game and voice volumes independently after recording
- Ring buffer stores mixed audio only

## Implementation Steps

### 1. Multi-Track Capture
- Capture system audio and mic as separate packet streams
- Do not mix them in the audio pipeline when multi-track mode is enabled
- Tag each packet with its source (system vs. mic) and track index

### 2. Ring Buffer Extension
- Extend ring buffer to store multiple audio streams
- Each snapshot includes video + system audio + mic audio packets
- Maintain synchronization across all streams via shared PTS

### 3. Multi-Track Muxing
- Update MP4 muxer to write multiple audio tracks
- Track 1: system audio, Track 2: mic audio
- Set appropriate metadata (language, title) per track
- Maintain stereo mixing as the default (backward compatible)

### 4. Configuration
- Add `separate_audio_tracks: bool` to `AudioConfig` (default: `false`)
- When enabled, record and export with separate tracks
- When disabled, mix as usual (current behavior)

### 5. Export Integration
- Gallery export respects the source track layout
- Option to export with separate tracks or mix down to stereo
- Per-track volume adjustment in export settings

### 6. GUI
- Add toggle in Audio settings: "Record separate audio tracks"
- Explain the use case (post-production editing)
- Show track layout preview

## Files to Modify
- `crates/liteclip-core/src/capture/audio/manager.rs` — Separate packet streams
- `crates/liteclip-core/src/capture/audio/mixer.rs` — Optional bypass of mixing
- `crates/liteclip-core/src/buffer/ring/spmc_ring.rs` — Multi-stream snapshot support
- `crates/liteclip-core/src/output/saver.rs` — Multi-track muxing
- `crates/liteclip-core/src/config/config_mod/types.rs` — Separate tracks config
- `crates/liteclip-core/src/output/mp4.rs` — Multi-track MP4 writing
- `src/gui/settings.rs` — Separate tracks toggle

## Estimated Effort
Large (5-8 days)

## Dependencies
- None

## Risks
- Memory usage doubles for audio (two streams instead of one mixed)
- Ring buffer complexity increases significantly
- Not all players support multi-track MP4 files
- Synchronization between tracks must be frame-accurate
