---
name: module-reviewer
description: Reviews a specific module for bugs, architecture, security, performance, and quality issues
---

# Module Reviewer

Reviews a specific code module comprehensively, producing structured findings.

## When to Use This Skill

Use for reviewing a specific module area of the codebase (e.g., capture, encode, buffer, output, gui, app, platform, config). Each module review produces a findings JSON file.

## Required Skills

None - this is pure code analysis using Read and Grep tools.

## Work Procedure

1. **Scope the module**: Read the feature description to identify exact paths to review. Use Glob to list all .rs files in the module directory.

2. **Deep read key files**: Read the main module files completely. Focus on:
   - Core abstractions (structs, traits, enums)
   - Public APIs and their contracts
   - Error handling patterns
   - Thread safety mechanisms
   - Resource management

3. **Pattern-based search**: Use Grep to find specific patterns in the module:
   - `.unwrap()` and `.expect()` - potential panic sites
   - `unsafe` blocks - safety invariants
   - `.clone()` - potential unnecessary allocations
   - `Mutex`, `RwLock`, `Atomic*` - concurrency patterns
   - `loop`, `while` - potential infinite loops or hot paths
   - Error types and conversions
   - `drop`, cleanup, close patterns

4. **Analyze findings**: For each issue found:
   - Determine severity (Critical, High, Medium, Low, Info)
   - Determine category (Bug, Architecture, Security, Performance, Quality)
   - Document file path, location, issue, impact, suggested fix

5. **Write findings**: Create `.factory/reviews/{module}_findings.json` with all findings.

## Severity Guidelines

| Severity | Criteria |
|----------|----------|
| **Critical** | Crashes, data loss, security vulnerabilities, memory corruption, deadlocks that actually occur |
| **High** | Significant bugs, major architectural flaws, resource leaks that grow over time |
| **Medium** | Quality issues that could cause problems, minor bugs under edge cases, unclear code |
| **Low** | Style issues, minor naming inconsistencies, small improvements |
| **Info** | Observations, patterns worth noting, potential future work |

## Example Handoff

```json
{
  "salientSummary": "Reviewed capture module (DXGI, WASAPI, backpressure). Found 2 High-severity bugs in DXGI error recovery, 3 Medium performance issues in audio capture, and several Low quality issues. Findings written to .factory/reviews/capture_findings.json with 12 total findings.",
  "whatWasImplemented": "Comprehensive review of crates/liteclip-core/src/capture/ directory (capture.rs, device.rs, texture.rs, audio/*.rs, backpressure.rs). Identified specific issues with file paths and locations.",
  "whatWasLeftUndone": "",
  "verification": {
    "commandsRun": [],
    "interactiveChecks": []
  },
  "tests": {
    "added": []
  },
  "discoveredIssues": [],
  "findingsFile": ".factory/reviews/capture_findings.json",
  "findingsCount": {
    "Critical": 0,
    "High": 2,
    "Medium": 3,
    "Low": 5,
    "Info": 2
  }
}
```

## When to Return to Orchestrator

- Cannot access required files (permissions, missing paths)
- Module scope is ambiguous or larger than expected
- Found critical issues that should be escalated immediately
