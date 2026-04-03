# Plan: Internationalization (i18n) and Localization

## Status
Pending

## Priority
Low

## Summary
Add internationalization support to LiteClip, enabling the GUI to be translated into multiple languages. Currently all UI text is hardcoded in English. Localization would make LiteClip accessible to non-English speakers worldwide.

## Current State
- All GUI text is hardcoded in English strings
- No i18n framework or translation infrastructure exists
- egui supports internationalization through translation crates
- No language selection option in settings

## Implementation Steps

### 1. i18n Framework
- Add `i18n-embed` and `fluent` (or `i18n-embed-fl`) crate dependencies
- Create translation files in Fluent format (`.ftl` files)
- Set up translation loading from embedded assets or files

### 2. String Extraction
- Extract all user-facing strings from GUI code
- Replace hardcoded strings with translation keys
- Organize strings by module (settings, gallery, tray, notifications)

### 3. Translation Files
- Create initial English translation file as the source
- Set up translation workflow for community contributions
- Use a translation management platform (e.g., Crowdin, Weblate) or GitHub-based workflow

### 4. Language Selection
- Add language dropdown to General settings tab
- Detect system language on first run
- Allow manual override
- Store language preference in config

### 5. Font Support
- Ensure egui fonts support non-Latin characters (CJK, Cyrillic, Arabic)
- Bundle fallback fonts for scripts not covered by the default font
- Handle right-to-left text (Arabic, Hebrew) if needed

### 6. Date/Time/Number Formatting
- Use locale-aware formatting for dates, times, numbers, and file sizes
- Respect the user's locale settings

## Files to Modify
- `Cargo.toml` — Add i18n dependencies
- `src/gui/settings.rs` — Extract all strings, add language selector
- `src/gui/gallery.rs` — Extract all strings
- `src/gui/gallery/browser.rs` — Extract all strings
- `src/gui/gallery/editor.rs` — Extract all strings
- `src/platform/tray.rs` — Extract tray menu strings
- `src/` — New `i18n/` module with translation files
- `crates/liteclip-core/src/config/config_mod/types.rs` — Language preference config

## Estimated Effort
Medium (3-5 days for infrastructure + ongoing translation work)

## Dependencies
- `i18n-embed`, `fluent` crates
- Community translators for non-English languages

## Risks
- Translation quality varies by contributor
- Some languages require significantly more UI space (German, Russian)
- RTL languages require layout changes
- Font bundling increases binary size
