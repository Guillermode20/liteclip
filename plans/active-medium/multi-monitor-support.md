# Plan: Multi-Monitor Support

## Status
Pending

## Priority
Medium

## Summary
Allow users to select which monitor to record from. Currently DXGI capture only captures a single output (hardcoded `output_index` defaulting to 0). Users with multiple monitors cannot choose which monitor to record.

## Current State
- Capture init iterates adapters but always uses `config.output_index` which defaults to 0
- `gpu_index` config selects the GPU adapter, not the monitor output
- No monitor enumeration or selection UI exists
- Multi-monitor setups are common for gaming and productivity

## Implementation Steps

### 1. Monitor Enumeration
- Enumerate all outputs per DXGI adapter
- Collect monitor metadata: name, resolution, refresh rate, primary flag
- Map each output to a stable identifier (not just index, which can change)

### 2. Configuration
- Add `monitor` field to `VideoConfig`: `Primary | Index(u32) | Name(String)`
- Serialize/deserialize in TOML config
- Default to `Primary` for backward compatibility

### 3. Capture Integration
- Update `DxgiCapture::new()` to accept a monitor selector
- Resolve the selector to the correct `IDXGIOutput` at init time
- Handle monitor disconnection gracefully (fall back to primary, show notification)

### 4. GUI
- Add monitor dropdown to the Video settings tab
- Show friendly names with resolution and refresh rate
- Indicate which monitor is primary
- Refresh list on settings open

### 5. Dynamic Monitor Changes
- Detect monitor hot-plug/unplug events
- Auto-switch if the selected monitor is disconnected
- Re-evaluate output list on display configuration change

## Files to Modify
- `crates/liteclip-core/src/config/config_mod/types.rs` — Add monitor selection config
- `crates/liteclip-core/src/capture/dxgi/capture.rs` — Multi-output enumeration and selection
- `crates/liteclip-core/src/capture/dxgi/device.rs` — Output resolution logic
- `src/gui/settings.rs` — Monitor dropdown in Video tab
- `src/main.rs` — Handle monitor change notifications

## Estimated Effort
Medium (3-5 days)

## Dependencies
- None (uses existing DXGI infrastructure)

## Risks
- Output indices can change between sessions if monitors are rearranged
- Mixed DPI scaling across monitors complicates coordinate calculations
- Some monitors may not support Desktop Duplication (e.g., remote displays)
