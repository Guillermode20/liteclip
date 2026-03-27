# Code Review Deduplication Patterns

**Date**: 2026-03-27
**Source**: synthesize-final-report feature review
**Milestone**: synthesis

## Key Observation

When running comprehensive code reviews with both module-level and cross-cut reviews, expect significant overlap in findings. The synthesis process must deduplicate findings to avoid redundancy in the final report.

## Overlap Pattern

Module-specific findings frequently appear in multiple review files:

| Finding Type | Appears In | Example Count |
|--------------|------------|---------------|
| GUI decode issues | gui_findings.json, threading_findings.json, performance_findings.json | 3 |
| Registry handle leak | platform_findings.json, security_findings.json | 2 |
| Encoder keyframe counting | encode_findings.json, performance_findings.json | 2 |
| Audio memory usage | gui_findings.json, performance_findings.json | 2 |
| Config fallback | config_findings.json, error_handling_findings.json | 2 |

## Deduplication Strategy

1. **Keep more detailed version**: When the same issue appears in multiple files, keep the version with more context and detail.
2. **Merge implications**: When cross-cut reviews add different perspective (e.g., threading impact on a GUI bug), merge that context into the kept version.
3. **Categorize appropriately**: Place deduplicated finding in the category that makes most sense (e.g., a threading issue in GUI code → Performance category).

## Statistics from LiteClip Review

- Raw findings: 173
- Deduplicated findings: 162
- Overlapping issues: 8
- Overlap sources typically: 2-3 files per overlapping issue

## Recommendations for Future Reviews

1. When designing review scope, consider whether module + cross-cut creates duplication
2. Document overlap expectations in synthesis-reviewer skill
3. Use structured JSON format for findings to enable automated deduplication
4. Track overlap_deduplication in summary file for transparency
