# Plan: Game Detection Improvements

## Status
Pending

## Priority
Medium

## Summary
Enhance the game detection system to be more accurate, support more games, and provide better fallback behavior. Currently game detection uses basic fullscreen window enumeration and may miss borderless windowed games or misidentify applications.

## Current State
- Game detection in `detection/game.rs` detects running fullscreen games
- Uses basic window enumeration
- May miss borderless windowed games
- No game database or heuristic matching
- Clips during non-game activity are saved to "Desktop" folder

## Implementation Steps

### 1. Borderless Windowed Detection
- Detect borderless windowed games (common in modern games)
- Check for fullscreen-style window properties:
  - Window size matches monitor resolution
  - No window decorations
  - Topmost z-order
- Cross-reference with known game executables

### 2. Game Database
- Build a local database of known game executables and window titles
- Include common games with their typical process names
- Allow user-contributed game entries
- Store database as an embedded JSON or TOML file

### 3. Heuristic Matching
- Match games by:
  - Process name (exact and fuzzy match)
  - Window title patterns (regex)
  - Executable path patterns
  - Window class names
- Confidence scoring: high confidence auto-detect, low confidence suggest to user

### 4. User Override
- Allow users to manually set the game name for a clip
- "This is not [detected game]" option in the gallery
- Remember user corrections for future detections
- Per-game settings (e.g., "Always treat X as a game")

### 5. Anti-Cheat Compatibility
- Detect when anti-cheat software is running (Easy Anti-Cheat, BattlEye, Vanguard)
- Adjust detection behavior to avoid conflicts
- Log warnings if detection may be blocked by anti-cheat

### 6. GUI
- Add game detection settings to General tab
- Show currently detected game in tray tooltip
- Game database management panel (add/remove/edit entries)

## Files to Modify
- `src/detection/game.rs` — Enhanced detection logic
- `src/detection/` — New `database.rs` module for game database
- `src/detection/` — New `heuristics.rs` module for matching
- `crates/liteclip-core/src/config/config_mod/types.rs` — Game detection config
- `src/main.rs` — Anti-cheat detection
- `src/gui/settings.rs` — Game detection settings

## Estimated Effort
Medium (3-5 days)

## Dependencies
- None

## Risks
- Game database requires ongoing maintenance
- Anti-cheat software may block process enumeration
- False positives (detecting non-games as games) are annoying
- Borderless windowed detection may catch non-game applications
