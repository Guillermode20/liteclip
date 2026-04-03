# Plan: Enhanced Gallery Search

## Status
Pending

## Priority
Medium

## Summary
Improve the gallery search with a unified query syntax supporting date ranges, resolution, duration, file size, and full-text metadata search. Currently search only matches filename and game name substrings.

## Current State
- Gallery search matches filename and game name substrings only
- Date range filter exists as a dropdown but cannot be combined with other filters
- No search by resolution, duration range, or file size
- Users with large clip libraries need powerful search

## Implementation Steps

### 1. Query Syntax
- Support a simple query language:
  - Free text: matches filename, game name
  - `game:"Valorant"` — Exact game name match
  - `date:2024-01-01..2024-01-31` — Date range
  - `duration:>60` or `duration:30..120` — Duration in seconds
  - `size:>100mb` — File size filter
  - `resolution:1080p` — Resolution filter
  - `codec:hevc` — Codec filter
  - Combined: `game:"Valorant" duration:>60 date:this_week`

### 2. Metadata Index
- Build an in-memory index of all clips with metadata:
  - Filename, game name, date, duration, size, resolution, codec
  - Index updates when clips are added or removed
- Use efficient data structures for range queries

### 3. Search Parser
- Parse query string into a filter tree
- Support AND/OR logic (implicit AND between terms)
- Handle quoted strings for exact matches
- Provide autocomplete suggestions as user types

### 4. GUI
- Replace current search input with enhanced search bar
- Show autocomplete dropdown with suggestions
- Display active filters as removable chips/tags
- Show result count and filter summary

### 5. Performance
- Index clips asynchronously on gallery open
- Incremental index updates for new/deleted clips
- Search results update in real-time as user types (debounced)

## Files to Modify
- `src/gui/gallery/browser.rs` — Enhanced search parser and UI
- `src/gui/gallery.rs` — Metadata index, combined filter logic
- `crates/liteclip-core/src/output/video_file.rs` — Extended metadata extraction
- `src/gui/gallery/types.rs` — Query and filter types

## Estimated Effort
Medium (3-5 days)

## Dependencies
- None

## Risks
- Query parser complexity (edge cases in syntax)
- Index memory usage for very large clip libraries
- Autocomplete performance with thousands of clips
