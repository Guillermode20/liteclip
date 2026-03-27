---
name: cross-cut-reviewer
description: Reviews cross-cutting patterns: security, performance, threading, error handling across all modules
---

# Cross-Cut Reviewer

Reviews patterns that span multiple modules: security issues, performance hotspots, threading correctness, error handling patterns.

## When to Use This Skill

Use for cross-cutting reviews that examine patterns across the entire codebase (security, performance, threading, error handling). These reviews complement module-specific reviews.

## Required Skills

None - this is pure code analysis using Read and Grep tools.

## Work Procedure

1. **Understand the cross-cut area**: Read the feature description to understand which cross-cut dimension to review (security, performance, threading, or error-handling).

2. **Pattern-based search across codebase**: Use Grep with path patterns to search across all source directories:
   - Security: `unsafe`, raw pointer operations, FFmpeg API calls, NULL checks, buffer bounds, input validation
   - Performance: `.clone()` in hot paths, allocations, tight loops, unnecessary work, polling patterns
   - Threading: `Mutex`, `RwLock`, `Atomic`, channel patterns, thread spawn, blocking calls
   - Error handling: `unwrap()`, `expect()`, error propagation, recovery patterns, panic sites

3. **Examine specific findings**: Read files around pattern matches to understand context. Determine if the pattern represents an actual issue.

4. **Categorize findings by module**: Organize findings by which module they appear in, but focus on the cross-cut pattern itself.

5. **Identify systemic patterns**: Look for issues that repeat across modules - these are systemic problems.

6. **Write findings**: Create `.factory/reviews/{cross-cut-area}_findings.json` with all findings, plus a patterns summary.

## Cross-Cut Areas

| Area | Key Patterns to Search |
|------|------------------------|
| **Security** | `unsafe`, `*mut`, `*const`, FFmpeg API calls, NULL pointers, bounds checks missing, input validation gaps |
| **Performance** | `.clone()` on large types, allocations in loops, unnecessary polling, redundant work, cache misses |
| **Threading** | `Mutex::lock()`, potential deadlock patterns, race conditions, thread spawn without join, blocking in async |
| **Error Handling** | `.unwrap()`, `.expect()`, error swallowed, recovery gaps, panic sites, inconsistent error types |

## User-Reported Issues

For PERFORMANCE review specifically: User mentioned "some excess cpu and memory usage, some related to the ui". Focus extra attention on:
- src/gui/ module: decode_pipeline, gallery, render patterns
- Memory allocation patterns in hot loops
- Potential render loops or unnecessary repaints

## Example Handoff

```json
{
  "salientSummary": "Performed security cross-cut review. Found 1 Critical issue (unsafe block with broken safety invariant), 4 High issues (FFmpeg NULL pointer risks), 8 Medium issues (missing bounds checks). Systemic pattern: insufficient validation of FFmpeg return values across multiple modules.",
  "whatWasImplemented": "Security pattern search across crates/liteclip-core/src/ and src/. Examined all unsafe blocks, FFmpeg API calls, and input validation points.",
  "whatWasLeftUndone": "",
  "verification": {
    "commandsRun": [],
    "interactiveChecks": []
  },
  "tests": {
    "added": []
  },
  "discoveredIssues": [],
  "findingsFile": ".factory/reviews/security_findings.json",
  "findingsCount": {
    "Critical": 1,
    "High": 4,
    "Medium": 8,
    "Low": 3,
    "Info": 2
  },
  "systemicPatterns": [
    "FFmpeg return values often unchecked for NULL",
    "Unsafe blocks lack documented safety invariants",
    "Config values validated inconsistently"
  ]
}
```

## When to Return to Orchestrator

- Cannot complete pattern search (tool limitations)
- Found critical security vulnerabilities requiring immediate escalation
- Discovered systemic patterns affecting mission scope
