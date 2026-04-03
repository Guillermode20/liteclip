# Plan: Automatic Updates

## Status
Pending

## Priority
Medium

## Summary
Implement an automatic update system that checks for new releases, downloads updates, and applies them with minimal user intervention. This reduces friction for users to stay on the latest version.

## Current State
- No update mechanism exists
- Users must manually download new releases from GitHub
- Version is tracked in `Cargo.toml` (`0.2.0`)
- No update check code exists anywhere in the codebase

## Implementation Steps

### 1. Update Check
- Poll GitHub Releases API for latest version
- Compare current version against latest release (semver comparison)
- Check on app startup (with cooldown -- max once per 24 hours)
- Support manual "Check for Updates" button in settings

### 2. Download
- Download the latest MSI installer to a temporary location
- Show download progress in the GUI
- Verify file integrity with SHA-256 checksum from release assets
- Support resuming interrupted downloads

### 3. Installation
- Launch the downloaded MSI installer with silent/quiet flags
- Preserve user configuration and clip library during upgrade
- Restart LiteClip after installation completes
- Handle installation failures gracefully (roll back, notify user)

### 4. Update Channel
- Support update channels: `stable` (default), `beta`, `nightly`
- Allow users to opt-in to pre-release updates in settings
- Respect the channel when checking GitHub Releases

### 5. Notifications
- Show toast notification when update is available
- Show changelog summary in the update dialog
- Allow users to defer the update (remind later)
- Critical updates can override deferral

### 6. Privacy
- Update check sends minimal data: current version, OS version, update channel
- No telemetry or usage data included
- Allow users to disable automatic update checks entirely

## Files to Modify
- `crates/liteclip-core/src/` -- New `updater/` module
- `crates/liteclip-core/src/config/config_mod/types.rs` -- Add update channel/config
- `src/gui/settings.rs` -- Add update settings and dialog
- `src/main.rs` -- Add update check on startup
- `src/platform/autostart.rs` -- May need to coordinate with update restart

## Estimated Effort
Medium (3-4 days)

## Dependencies
- GitHub Releases API (no authentication needed for public repos)
- MSI installer supports silent installation (`msiexec /quiet`)

## Risks
- Update download/installation could fail, leaving user in broken state
- Must handle corrupted downloads and interrupted installations
- Code signing is important for user trust during updates
- Silent MSI install may not show errors to the user
