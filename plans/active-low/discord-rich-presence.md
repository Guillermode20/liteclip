# Plan: Discord Rich Presence Integration

## Status
Pending

## Priority
Low

## Summary
Add Discord Rich Presence (RPC) integration so Discord can show "Recording with LiteClip" or "Editing clip in LiteClip" as a rich presence status. This is a lightweight feature with high perceived value for social gamers.

## Current State
- No Discord RPC integration exists
- No presence or activity status is set
- Discord users cannot see LiteClip activity from their friends list

## Implementation Steps

### 1. Discord RPC Setup
- Add `discord-rich-presence` or `discord-rpc` crate dependency
- Initialize RPC client with LiteClip application ID
- Register application on Discord Developer Portal (if not already done)
- Set up application assets (icons for "Recording", "Editing", "Idle")

### 2. Presence States
- **Idle**: "Browsing LiteClip" with app logo
- **Recording**: "Recording gameplay" with red recording icon, show elapsed time
- **Paused**: "Recording paused" with yellow pause icon
- **Editing**: "Editing clips in Gallery" with edit icon, show clip count
- **Exporting**: "Exporting clip" with progress percentage

### 3. Integration Points
- Update presence on recording state changes (start, stop, pause)
- Update presence when gallery opens/closes
- Update presence during export with progress
- Update presence on tray menu interactions

### 4. Configuration
- Add `discord_rpc_enabled: bool` to `GeneralConfig` (default: `true`)
- Allow users to disable RPC in settings
- Handle Discord not installed gracefully (no-op, no errors)

### 5. Error Handling
- Discord client may not be running — handle connection failures silently
- RPC pipe may be unavailable — log warning, continue without RPC
- No impact on core recording functionality if RPC fails

## Files to Modify
- `Cargo.toml` — Add Discord RPC crate dependency
- `src/main.rs` — RPC initialization and state updates
- `src/gui/gallery.rs` — Update presence when gallery is open
- `crates/liteclip-core/src/config/config_mod/types.rs` — Add RPC enable/disable config
- `src/platform/tray.rs` — Update presence on tray interactions

## Estimated Effort
Small (1-2 days)

## Dependencies
- Discord client installed (optional, graceful degradation)
- Discord Developer Portal application registration

## Risks
- Discord RPC protocol may change between Discord versions
- Some users may not want their activity broadcast to Discord
- RPC connection can be flaky on some systems
