---
name: bug-fix-worker
description: Fixes bugs identified in code review with TDD approach
---

# Bug Fix Worker

Fixes bugs identified in the code review using TDD approach.

## When to Use This Skill

Use for fixing specific bugs like keyframe counting, unwrap panics, thread lifecycle issues, handle leaks, etc.

## Required Skills

None - uses Read, Edit, Execute tools.

## Work Procedure

1. **Read the bug details**: Use Read tool to understand the file and location from the feature description.

2. **Write failing test (TDD)**: 
   - Create or modify test file
   - Write test that demonstrates the bug
   - Run test to verify it fails: `cargo test --test <test_name>`
   - Commit test file separately

3. **Implement the fix**:
   - Use Edit tool to fix the bug
   - Run the failing test to verify it passes
   - Run full test suite: `cargo test --workspace`

4. **Verify no regressions**:
   - `cargo check --workspace`
   - `cargo clippy --workspace -- -D warnings`

5. **Commit the fix**:
   - Stage and commit with descriptive message

## Example Handoff

```json
{
  "salientSummary": "Fixed keyframe counting bug in all encoder paths. Changed `frame_count` to `encoder_frame_count` in 4 files. Added test verifying GOP alignment. All tests pass.",
  "whatWasImplemented": "Fixed encode/ffmpeg/mod.rs:318, nvenc.rs:140, amf.rs:158, qsv.rs:203. All now use encoder_frame_count for keyframe decisions. Added test in encode/tests/keyframe_test.rs.",
  "whatWasLeftUndone": "",
  "verification": {
    "commandsRun": [
      {"command": "cargo test keyframe", "exitCode": 0, "observation": "Test passes"},
      {"command": "cargo test --workspace", "exitCode": 0, "observation": "All 84 tests pass"},
      {"command": "cargo clippy -- -D warnings", "exitCode": 0, "observation": "No warnings"}
    ],
    "interactiveChecks": []
  },
  "tests": {
    "added": [{"file": "tests/keyframe_test.rs", "cases": [{"name": "test_gop_alignment", "verifies": "VAL-FIX-001"}]}]
  },
  "discoveredIssues": [],
  "fulfills": ["VAL-FIX-001"]
}
```

## When to Return to Orchestrator

- Cannot write meaningful test (needs discussion)
- Fix requires architectural changes beyond scope
- Dependencies missing or broken
- Test suite fails unrelated to fix
