---
name: gui-worker
description: Handles GUI thread modifications for CPU reduction - EventLoopProxy integration, conditional repaint logic
---

# GUI Worker

NOTE: Startup and cleanup are handled by `worker-base`. This skill defines the WORK PROCEDURE.

## When to Use This Skill

Use for features that modify the GUI thread behavior:
- EventLoopProxy wake mechanism implementation
- Conditional repaint logic changes
- Dormancy activation logic
- Toast/window timing handling

## Required Skills

None. This worker makes code changes to the GUI manager module.

## Work Procedure

### 1. Read Research and Understand Context

Read the following files before making any changes:
- `.factory/research/winit-threading.md` - winit constraints and workarounds
- `.factory/research/eframe-on-demand.md` - eframe integration patterns
- `.factory/research/gui-cpu-analysis.md` - current CPU usage analysis
- `mission.md` - mission objectives
- `AGENTS.md` - thread boundaries and off-limits areas

### 2. Understand Current Implementation

Read the current GUI manager implementation:
- `src/gui/manager.rs` - full file
- Identify the problematic code at line 430-433 (periodic repaint)
- Understand the channel polling mechanism
- Understand how Settings/Gallery are spawned

### 3. Write Tests First (TDD)

Before implementing changes, write unit tests for:
- Idle state detection logic
- Conditional repaint decision making
- Dormancy activation timing

Test file location: Create tests in `src/gui/` as inline `#[cfg(test)]` modules.

Run tests to ensure they FAIL (red phase):
```bash
cargo test --workspace
```

### 4. Implement Changes

Make targeted modifications following the approach documented in research:
- Only modify files in `src/gui/manager.rs` and related GUI modules
- DO NOT touch recording pipeline, platform thread, or capture/encode code
- Use `EventLoopProxy` for wake-on-message (see eframe-on-demand.md)
- Implement conditional repaint logic that stops when truly idle

### 5. Verify Tests Pass

Run tests to ensure implementation makes tests PASS (green phase):
```bash
cargo test --workspace
```

If tests fail, debug and fix before proceeding.

### 6. Run Validators

Run all validation commands:
```bash
cargo check --workspace
cargo clippy --workspace -- -D warnings
cargo fmt --check
```

Fix any issues before proceeding.

### 7. Manual Verification

Build release and perform quick sanity check:
```bash
cargo build --release --features ffmpeg
```

Run application briefly to verify:
- Application starts without crash
- Settings opens from tray
- Gallery opens from tray/hotkey
- Toast appears when clip saved

Note any issues in handoff.

### 8. Commit Changes

Commit with descriptive message:
```bash
git add -A
git commit -m "[descriptive message about GUI changes]"
```

## Example Handoff

```json
{
  "salientSummary": "Implemented EventLoopProxy wake mechanism and conditional repaint logic in manager.rs. Removed 100ms periodic repaint when truly idle, added EventLoopProxy to wake on GuiMessage arrival. Tests pass, manual verification shows app starts correctly.",
  "whatWasImplemented": "Modified src/gui/manager.rs to: (1) hold EventLoopProxy in GuiManagerState, (2) send wake event when GuiMessage arrives, (3) stop request_repaint_after when no windows/toasts visible. Added unit tests for idle detection logic in inline test module.",
  "whatWasLeftUndone": "",
  "verification": {
    "commandsRun": [
      { "command": "cargo test --workspace", "exitCode": 0, "observation": "All 15 tests pass, including new idle detection tests" },
      { "command": "cargo check --workspace", "exitCode": 0, "observation": "No type errors" },
      { "command": "cargo clippy --workspace -- -D warnings", "exitCode": 0, "observation": "No clippy warnings" },
      { "command": "cargo fmt --check", "exitCode": 0, "observation": "Formatting correct" },
      { "command": "cargo build --release --features ffmpeg", "exitCode": 0, "observation": "Release build successful" }
    ],
    "interactiveChecks": [
      { "action": "Started app from tray icon", "observed": "App started, tray icon appeared" },
      { "action": "Right-click tray → Settings", "observed": "Settings window opened within ~150ms" },
      { "action": "Press Save Clip hotkey", "observed": "Toast appeared within ~50ms" }
    ]
  },
  "tests": {
    "added": [
      { "file": "src/gui/manager.rs", "cases": [
        { "name": "test_is_truly_idle", "verifies": "Idle detection when no windows/toasts visible" },
        { "name": "test_should_request_repaint", "verifies": "Conditional repaint logic" }
      ]}
    ]
  },
  "discoveredIssues": []
}
```

## When to Return to Orchestrator

- EventLoopProxy integration requires changes to eframe internal APIs not accessible
- Tests consistently fail after multiple fix attempts
- Manual verification shows application crash or critical regression
- Thread boundaries need to be violated (would require orchestrator approval)
- Platform-specific behavior discovered that affects approach
