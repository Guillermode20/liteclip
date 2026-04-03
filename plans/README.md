# LiteClip Development Plans

Central repository of future development plans for LiteClip. Each plan is a standalone markdown file with implementation details, effort estimates, and risk assessments.

## Folders

| Folder | Purpose |
|--------|---------|
| [`active-high/`](active-high/) | Urgent plans to work on now |
| [`active-medium/`](active-medium/) | Important but not urgent |
| [`active-low/`](active-low/) | Nice-to-have or long-term plans |
| [`completed/`](completed/) | Plans that have been implemented |

## Active — High Priority (5)

| Plan | Effort | Summary |
|------|--------|---------|
| [Screenshot Hotkey](active-high/screenshot-hotkey.md) | Medium (3-5 days) | Implement the already-configured screenshot hotkey handler |
| [H.264 and AV1 Codec Support](active-high/h264-av1-codec-support.md) | Large (5-8 days) | Add H.264 and AV1 encoding alongside existing HEVC |
| [NVENC and QSV Encoder Testing](active-high/nvenc-qsv-encoder-testing.md) | Medium (3-5 days) | Verify NVIDIA and Intel hardware encoder paths on real hardware |
| [Pause/Resume Recording](active-high/pause-resume-recording.md) | Large (5-8 days) | Pause recording without losing the replay buffer |
| [Crash Recovery](active-high/crash-recovery.md) | Medium (3-5 days) | Atomic writes and corrupted clip detection/repair |

## Active — Medium Priority (16)

| Plan | Effort | Summary |
|------|--------|---------|
| [Custom System Audio Device](active-medium/custom-system-audio-device.md) | Medium (2-3 days) | Allow selecting specific audio output device for capture |
| [Filter-Graph Export Pipeline](active-medium/filter-graph-export.md) | Large (5-7 days) | Frame-accurate trimming with FFmpeg filter graphs |
| [Installer Enhancements](active-medium/installer-enhancements.md) | Medium (3-5 days) | Code signing, FFmpeg bootstrapper, better install UX |
| [Region/Area Selection](active-medium/region-selection.md) | Large (5-7 days) | Record specific screen regions or individual windows |
| [Automatic Updates](active-medium/automatic-updates.md) | Medium (3-4 days) | Auto-check, download, and apply updates from GitHub Releases |
| [Multi-Monitor Support](active-medium/multi-monitor-support.md) | Medium (3-5 days) | Select which monitor to record from |
| [Cursor Visibility Toggle](active-medium/cursor-visibility-toggle.md) | Small (1-2 days) | Show or hide the mouse cursor in recordings |
| [Recording Indicator Overlay](active-medium/recording-indicator-overlay.md) | Medium (3-5 days) | On-screen "REC" indicator for fullscreen confidence |
| [Custom Filename Templates](active-medium/custom-filename-templates.md) | Small (1-2 days) | Customizable output filename patterns |
| [Webhook Integration](active-medium/webhook-notification-integration.md) | Medium (3-5 days) | Post-save notifications to Discord and other services |
| [Clipboard Integration](active-medium/clipboard-integration.md) | Small (1-2 days) | Auto-copy clip path to clipboard on save |
| [Batch Export](active-medium/batch-export.md) | Medium (3-5 days) | Export multiple clips at once from the gallery |
| [Enhanced Gallery Search](active-medium/enhanced-gallery-search.md) | Medium (3-5 days) | Unified query syntax with date, size, duration filters |
| [Encoder Warmup](active-medium/encoder-warmup.md) | Small (1-2 days) | Stabilize rate control before first recorded frame |
| [Log Rotation](active-medium/log-rotation.md) | Small (1-2 days) | Prevent unbounded log file growth |
| [Game Detection Improvements](active-medium/game-detection-improvements.md) | Medium (3-5 days) | Borderless windowed detection, game database, heuristics |

## Active — Low Priority (16)

| Plan | Effort | Summary |
|------|--------|---------|
| [Two-Pass Software Encoding](active-low/two-pass-software-encoding.md) | Medium (2-3 days) | Better quality exports at target file sizes |
| [Cross-Platform Support](active-low/cross-platform-support.md) | Very Large (3-6 weeks/platform) | macOS and Linux support |
| [Audio Waveform Visualization](active-low/audio-waveform-visualization.md) | Medium (3-4 days) | Waveform display in timeline and gallery |
| [Performance Dashboard](active-low/performance-dashboard.md) | Medium-Large (4-6 days) | Real-time GPU, memory, and encoder metrics |
| [Scheduled Recording](active-low/scheduled-recording.md) | Medium (3-5 days) | Start/stop recording on a schedule |
| [Discord Rich Presence](active-low/discord-rich-presence.md) | Small (1-2 days) | Show LiteClip activity in Discord status |
| [Encoder Advanced Options](active-low/encoder-advanced-options.md) | Medium (3-5 days) | NVENC/QSV/AMF-specific tuning parameters |
| [Separate Audio Tracks](active-low/separate-audio-tracks.md) | Large (5-8 days) | Export with system and mic on separate audio tracks |
| [Silent Installation](active-low/silent-installation-deployment.md) | Small (1-2 days) | Pre-configured deployment for enterprises |
| [Gallery Metadata Panel](active-low/gallery-metadata-panel.md) | Small (1-2 days) | Detailed codec, bitrate, and technical info per clip |
| [Gallery Keyboard Navigation](active-low/gallery-keyboard-navigation.md) | Medium (3-5 days) | Arrow-key browsing, shortcuts, screen reader support |
| [Continuous Recording Mode](active-low/continuous-recording-mode.md) | Large (5-8 days) | Direct-to-disk recording for long sessions |
| [Benchmark Expansion](active-low/benchmark-scenario-expansion.md) | Medium (3-5 days) | More scenarios for regression prevention |
| [Memory Recommendation](active-low/automatic-memory-recommendation.md) | Small (1-2 days) | Dynamic memory limit suggestions based on usage |
| [Internationalization](active-low/internationalization.md) | Medium (3-5 days) | Multi-language GUI support |
| [GUI Theming](active-low/gui-theming.md) | Medium (3-5 days) | Light/dark themes, accent colors, UI scaling |

## Completed

_No plans completed yet._

## Plan Format

Each plan file follows this structure:

- **Status** -- Current state (Pending, In Progress, Completed, Cancelled)
- **Priority** -- High, Medium, or Low
- **Summary** -- One-paragraph description
- **Current State** -- What exists today
- **Implementation Steps** -- Ordered breakdown of work
- **Files to Modify** -- Specific paths that will change
- **Estimated Effort** -- Time estimate
- **Dependencies** -- Prerequisites
- **Risks** -- Known challenges and concerns
