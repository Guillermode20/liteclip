---
name: architecture-fix-worker
description: Fixes architectural issues - mutex consolidation, config versioning, thread patterns
---

# Architecture Fix Worker

Fixes architectural issues identified in code review: mutex consolidation, config versioning, thread patterns.

## When to Use This Skill

Use for architectural fixes: consolidating mutexes, adding config versioning, improving thread patterns.

## Required Skills

None - uses Read, Edit, Execute tools.

## Work Procedure

1. **Read the architecture issue**: Use Read tool to understand the current structure.

2. **Design the fix**:
   - What's the target structure?
   - What's the minimal change?
   - What tests verify the change?

3. **Implement incrementally**:
   - Make small, focused changes
   - Run tests after each change
   - Ensure no regressions

4. **Add tests if applicable**:
   - Migration tests for config versioning
   - Concurrency tests for mutex changes

5. **Verify the fix**:
   - `cargo test --workspace`
   - `cargo clippy --workspace -- -D warnings`

6. **Commit the fix**:
   - Stage and commit with descriptive message

## Example Handoff

```json
{
  "salientSummary": "Added config_version field and migration logic. Consolidated 7 playback mutexes into 2. All tests pass.",
  "whatWasImplemented": "Added config_version: u32 to Config struct. Added migrate_config() function. Refactored SharedPlaybackState from 7 Mutex fields to Mutex<PlaybackState> struct.",
  "whatWasLeftUndone": "",
  "verification": {
    "commandsRun": [
      {"command": "cargo test config_migration", "exitCode": 0, "observation": "Migration tests pass"},
      {"command": "cargo test --workspace", "exitCode": 0, "observation": "All tests pass"},
      {"command": "cargo clippy -- -D warnings", "exitCode": 0, "observation": "No warnings"}
    ],
    "interactiveChecks": []
  },
  "tests": {
    "added": [{"file": "tests/config_migration_test.rs", "cases": [{"name": "test_migration_v0_to_v1", "verifies": "VAL-FIX-014"}]}]
  },
  "discoveredIssues": [],
  "fulfills": ["VAL-FIX-013", "VAL-FIX-014"]
}
```

## When to Return to Orchestrator

- Architecture change is too large for single feature
- Requires breaking API changes
- Dependencies on other unfixed issues
