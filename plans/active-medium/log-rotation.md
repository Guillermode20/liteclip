# Plan: Log Rotation and Size Management

## Status
Pending

## Priority
Medium

## Summary
Add log rotation to prevent unbounded log file growth. Currently the application logs to stdout/stderr with no rotation mechanism. Long-running instances can generate significant log output that consumes disk space.

## Current State
- Logging uses `tracing-subscriber` with `env-filter`
- Logs go to stdout/stderr only
- No built-in log file management
- No log rotation or size limits
- Users redirecting output to files have no automatic cleanup

## Implementation Steps

### 1. Log File Configuration
- Add `LogConfig` struct to `AdvancedConfig`:
  - `file_logging_enabled: bool` (default: `false`)
  - `log_directory: String` (default: `%APPDATA%\liteclip\logs`)
  - `max_file_size_mb: u64` (default: `10`)
  - `max_files: u32` (default: `5`)
  - `rotation: Never | Daily | Size` — Rotation strategy

### 2. Log Rotation Implementation
- Add `tracing-appender` dependency for file-based logging
- Configure rolling file appender with size or time-based rotation
- Delete oldest log files when `max_files` limit is reached
- Compress rotated log files (optional, `.gz`)

### 3. Log Format
- Include timestamp, level, target, and message
- Consistent format for easy parsing by external tools
- Include LiteClip version in log header

### 4. GUI
- Add log settings to the Advanced tab
- "Open Logs Folder" button for easy access
- "Export Logs" button to create a zip of recent logs for bug reports
- Show current log file size

### 5. Startup Log Cleanup
- On startup, delete log files older than `max_files` or `max_age_days`
- Log the cleanup action

## Files to Modify
- `src/main.rs` — Log file configuration with `tracing-appender`
- `Cargo.toml` — Add `tracing-appender` dependency
- `crates/liteclip-core/src/config/config_mod/types.rs` — Log rotation config in Advanced
- `src/gui/settings.rs` — Log settings UI

## Estimated Effort
Small (1-2 days)

## Dependencies
- `tracing-appender` crate

## Risks
- Log rotation must not lose log entries during rotation
- Compressed logs may be harder to read for debugging
- Log directory cleanup must handle locked files (active log)
