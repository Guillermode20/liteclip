---
name: synthesis-reviewer
description: Collects all module and cross-cut findings, deduplicates, assigns consistent severity, writes final markdown report
---

# Synthesis Reviewer

Collects all findings from module and cross-cut reviews, produces the final comprehensive markdown report.

## When to Use This Skill

Use as the final review step to synthesize all findings into the final deliverable: REVIEW_REPORT.md.

## Required Skills

None - this is analysis and documentation synthesis.

## Work Procedure

1. **Collect all findings**: Read all JSON files in `.factory/reviews/` directory. Parse each findings file.

2. **Deduplicate findings**: Cross-cut reviews and module reviews may find the same issue. Deduplicate by:
   - File path + location match
   - Same issue description
   - Keep the more detailed description, merge insights

3. **Review severity consistency**: Ensure severity ratings are consistent across findings:
   - Critical: crashes, data loss, security vulns, memory corruption
   - High: significant bugs, architectural flaws, resource leaks
   - Medium: quality issues, minor bugs, unclear code
   - Low: style, naming, small improvements
   - Info: observations, patterns

4. **Organize by severity and category**: Group findings by:
   - Primary: Severity (Critical → Low)
   - Secondary: Category (Bug, Architecture, Security, Performance, Quality)

5. **Identify cross-cutting patterns**: From cross-cut reviews, extract systemic patterns that span modules. These become a separate section.

6. **Write the final report**: Create `REVIEW_REPORT.md` in repository root with:
   - Executive summary (total findings per severity)
   - Critical findings section
   - High findings section
   - Medium findings section
   - Low/Info findings section
   - Cross-cutting patterns section
   - Recommendations for prioritization

7. **Write findings summary JSON**: Create `.factory/reviews/all_findings_summary.json` with aggregated statistics.

## Report Structure

```markdown
# LiteClip Recorder Code Review Report

## Executive Summary
- Total findings: X
- Critical: X, High: X, Medium: X, Low: X, Info: X
- Key systemic patterns identified

## Critical Findings
### [BUG] - Title
**File**: path
**Location**: function
**Issue**: description
**Impact**: what this causes
**Fix**: how to address

... (same format for High, Medium, Low, Info)

## Cross-Cutting Patterns
- Pattern 1: description, modules affected
- Pattern 2: description, modules affected

## Recommendations
1. Priority order for addressing findings
2. Quick wins vs. significant changes
3. Areas needing deeper investigation
```

## Example Handoff

```json
{
  "salientSummary": "Synthesized all findings from 8 module reviews and 4 cross-cut reviews. Deduplicated 3 overlapping findings. Produced REVIEW_REPORT.md with 47 findings (0 Critical, 5 High, 18 Medium, 19 Low, 5 Info). Identified 4 systemic cross-cutting patterns.",
  "whatWasImplemented": "Collected .factory/reviews/*.json files, deduplicated, organized by severity/category, wrote REVIEW_REPORT.md and all_findings_summary.json.",
  "whatWasLeftUndone": "",
  "verification": {
    "commandsRun": [],
    "interactiveChecks": []
  },
  "tests": {
    "added": []
  },
  "discoveredIssues": [],
  "reportFile": "REVIEW_REPORT.md",
  "summaryFile": ".factory/reviews/all_findings_summary.json",
  "totalFindings": 47,
  "findingsBySeverity": {
    "Critical": 0,
    "High": 5,
    "Medium": 18,
    "Low": 19,
    "Info": 5
  },
  "crossCuttingPatterns": 4
}
```

## When to Return to Orchestrator

- Missing findings files from expected reviewers
- Severity inconsistencies requiring orchestrator decision
- Report structure doesn't match validation contract requirements
