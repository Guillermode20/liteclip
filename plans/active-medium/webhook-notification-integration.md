# Plan: Webhook and Notification Integration

## Status
Pending

## Priority
Medium

## Summary
Add webhook support to notify external services after saving a clip. This enables automation workflows like posting clips to Discord channels, uploading to cloud storage, or triggering custom post-save scripts.

## Current State
- After saving a clip, the file path is logged but no external notification occurs
- No mechanism exists to trigger external actions on clip save
- Streamers and content creators must manually share or process clips

## Implementation Steps

### 1. Webhook Configuration
- Add `WebhookConfig` struct to config:
  - `enabled: bool`
  - `url: String` — Webhook endpoint URL
  - `method: Post | Put` — HTTP method
  - `payload_template: String` — JSON payload template with variables
  - `headers: Vec<(String, String)>` — Custom headers (e.g., Authorization)
  - `timeout_secs: u64` — Request timeout

### 2. Payload Variables
- `{file_path}` — Absolute path to saved clip
- `{file_name}` — Filename only
- `{file_size}` — File size in bytes
- `{duration_secs}` — Clip duration
- `{game}` — Detected game name
- `{resolution}` — Video resolution
- `{timestamp}` — ISO 8601 timestamp
- `{encoder}` — Encoder used

### 3. Webhook Dispatch
- Send webhook asynchronously after clip save completes
- Do not block the recording pipeline on webhook response
- Retry on failure (configurable: 0-3 retries)
- Log success/failure with response status

### 4. Discord Webhook Preset
- Provide a built-in Discord webhook template
- Auto-format as Discord embed with clip info and thumbnail
- One-click setup: paste Discord webhook URL, done

### 5. Post-Save Script Hook
- Optional: run a custom script/command after clip save
- Pass clip path as argument
- Useful for custom processing, uploads, or notifications

### 6. GUI
- Add webhook configuration to General or Advanced settings tab
- Test webhook button (sends a test payload)
- Show last webhook result (success/failure, status code)

## Files to Modify
- `crates/liteclip-core/src/config/config_mod/types.rs` — Add webhook config struct
- `crates/liteclip-core/src/app/clip.rs` — Webhook dispatch after save
- `crates/liteclip-core/src/output/saver.rs` — Post-save hook integration
- `Cargo.toml` — Add HTTP client dependency (e.g., `reqwest`)
- `src/gui/settings.rs` — Webhook config UI

## Estimated Effort
Medium (3-5 days)

## Dependencies
- HTTP client crate (e.g., `reqwest` with `blocking` or `async`)

## Risks
- Webhook failures should not impact recording performance
- Sensitive data in webhook URLs (tokens) must not be logged
- Network issues could cause delays if not handled asynchronously
