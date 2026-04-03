# Plan: Silent Installation and Deployment Configuration

## Status
Pending

## Priority
Low

## Summary
Enable pre-configuration of LiteClip settings during deployment. IT administrators, esports venues, and gaming cafes need to deploy LiteClip with pre-configured settings across many machines without manual setup.

## Current State
- MSI installer supports silent install (`msiexec /quiet`)
- No mechanism to pre-configure settings during deployment
- Config file is created on first run with defaults
- Each machine requires manual configuration after install

## Implementation Steps

### 1. CLI Arguments for Config
- Support `--config <path>` argument to load config from a specific file
- Support `--import-config <path>` to import settings into the user config directory
- Support `--show-config` to print current effective configuration

### 2. Bundled Config File
- Support reading a `config.toml` from the application directory on first run
- If user config does not exist, use bundled config as the initial settings
- Bundled config takes precedence over defaults but is overridden by user config

### 3. MSI Custom Action
- Add MSI custom action to deploy a config file during installation
- Config file path configurable via MSI property: `LITECLIP_CONFIG`
- Deploy to `%APPDATA%\liteclip\config.toml` during silent install

### 4. Environment Variable Support
- Support `LITECLIP_CONFIG` environment variable pointing to a config file
- Useful for containerized or managed deployments
- Override specific settings via `LITECLIP_<SETTING>` environment variables

### 5. Deployment Documentation
- Document the deployment workflow
- Provide example config files for common scenarios:
  - Esports venue (fixed hotkeys, specific encoder settings)
  - Gaming cafe (shared save directory, restricted settings)
  - Enterprise (disabled auto-start, specific output path)

## Files to Modify
- `crates/liteclip-core/src/config/config_mod/functions.rs` — Config from CLI args or bundled file
- `src/main.rs` — CLI argument parsing for config path
- `installer/` — MSI custom action for config deployment
- `crates/liteclip-core/src/paths.rs` — Support alternate config paths

## Estimated Effort
Small (1-2 days)

## Dependencies
- None

## Risks
- MSI custom actions require elevated privileges
- Environment variable parsing must handle edge cases
- Bundled config must not override user changes on subsequent runs
