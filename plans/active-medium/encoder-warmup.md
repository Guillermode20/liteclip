# Plan: Encoder Warmup and First-Frame Optimization

## Status
Pending

## Priority
Medium

## Summary
Add an encoder warmup period to stabilize rate control before the first recorded frame. The first few seconds of a clip may have visible quality artifacts if the encoder's rate control has not converged.

## Current State
- Encoder initializes on pipeline start
- First frames may have suboptimal quality while rate control converges
- No warmup or pre-roll encoding exists
- VBR and CQ modes are most affected by initial rate control instability

## Implementation Steps

### 1. Warmup Frame Encoding
- On encoder initialization, encode and discard a brief warmup period (1-2 seconds)
- Feed black frames or duplicate the first real frame
- Allow rate control to converge before recording begins
- Warmup duration configurable (default: 1 second)

### 2. Warmup Configuration
- Add `encoder_warmup_secs: f32` to `VideoConfig` (default: `1.0`, range: `0.0-5.0`)
- Set to `0.0` to disable warmup (for low-latency scenarios)
- Only applies to VBR and CQ rate control modes (CBR is less affected)

### 3. Integration with Replay Buffer
- Warmup frames are encoded but not added to the ring buffer
- The ring buffer starts receiving frames after warmup completes
- User-visible recording delay is the warmup duration

### 4. Pipeline Restart
- Warmup runs on every pipeline restart (config change, resume from pause)
- Log warmup completion: "Encoder warmup complete (X frames encoded)"

### 5. GUI
- Add warmup duration setting to the Advanced settings tab
- Explain the purpose: "Improves first-frame quality at the cost of a brief delay"

## Files to Modify
- `crates/liteclip-core/src/app/pipeline/video.rs` — Warmup frame encoding
- `crates/liteclip-core/src/encode/mod.rs` — Encoder warmup API
- `crates/liteclip-core/src/config/config_mod/types.rs` — Warmup duration config
- `src/gui/settings.rs` — Warmup setting in Advanced tab

## Estimated Effort
Small (1-2 days)

## Dependencies
- None

## Risks
- Warmup adds latency to recording start (configurable)
- Warmup frames consume GPU/CPU resources briefly
- Not all encoders benefit equally from warmup
