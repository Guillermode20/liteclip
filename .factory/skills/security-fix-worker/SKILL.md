---
name: security-fix-worker
description: Fixes security issues - adds SAFETY comments, NULL checks, validation
---

# Security Fix Worker

Fixes security issues identified in code review: SAFETY comments, NULL validation, input validation.

## When to Use This Skill

Use for fixing security issues: adding SAFETY documentation, NULL pointer checks, input validation.

## Required Skills

None - uses Read, Edit, Grep tools.

## Work Procedure

1. **Read the security issue details**: Use Read tool to understand the file and location.

2. **Analyze the unsafe pattern**:
   - What makes it unsafe?
   - What invariant ensures safety?
   - What check is missing?

3. **Add SAFETY comment or fix**:
   - For valid unsafe: Add `// SAFETY:` comment explaining invariant
   - For missing check: Add null check or validation
   - For invalid unsafe: Refactor to safe code if possible

4. **Verify the fix**:
   - `cargo check --workspace`
   - `cargo clippy --workspace -- -D warnings`
   - `cargo test --workspace`

5. **Commit the fix**:
   - Stage and commit with descriptive message

## SAFETY Comment Format

```rust
// SAFETY: <explanation of why this is safe>
// - <precondition 1>
// - <precondition 2>
unsafe {
    // unsafe operation
}
```

## Example Handoff

```json
{
  "salientSummary": "Added SAFETY comments to all FFmpeg unsafe blocks in encode module. Added NULL validation in context.rs hardware frame handling. 12 files modified.",
  "whatWasImplemented": "Added SAFETY comments to encode/ffmpeg/mod.rs:102, context.rs:148,186-189. Added null check after av_hwframe_get_buffer. All unsafe blocks now documented.",
  "whatWasLeftUndone": "",
  "verification": {
    "commandsRun": [
      {"command": "cargo check", "exitCode": 0, "observation": "Compiles"},
      {"command": "cargo clippy -- -D warnings", "exitCode": 0, "observation": "No warnings"},
      {"command": "cargo test", "exitCode": 0, "observation": "All tests pass"}
    ],
    "interactiveChecks": []
  },
  "tests": {
    "added": []
  },
  "discoveredIssues": [],
  "fulfills": ["VAL-FIX-008", "VAL-FIX-009"]
}
```

## When to Return to Orchestrator

- Unsafe pattern is fundamentally broken (needs refactor)
- Cannot determine safety invariant
- Change breaks other code
