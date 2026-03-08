# GPU Zero-Copy Video Plan

## Objective

Remove the remaining CPU work from the Windows video path where it materially affects realtime capture throughput.

The current recorder already does these steps on the GPU:

- DXGI desktop capture
- optional GPU downscale from native desktop resolution to target output resolution
- hardware video encode selection and use of AMD AMF when available

The remaining CPU work in the AMD path is:

- staging-texture readback from D3D11 to system memory
- BGRA byte copy into FFmpeg software frames
- CPU-side BGRA -> NV12 conversion via `swscale`

The next goal is to replace that with:

- DXGI capture into D3D11 textures
- optional GPU scaling into a D3D11 render target
- GPU color conversion to NV12
- FFmpeg/AMF ingest of D3D11 NV12 hardware frames

## Current State

### What is already in place

- Native FFmpeg encoder path is active.
  - Evidence: [src/encode/ffmpeg_encoder.rs](c:/Coding/liteclip-recorder/src/encode/ffmpeg_encoder.rs)
- DXGI capture already supports GPU scaling before readback.
  - Evidence: [src/capture/dxgi/types.rs](c:/Coding/liteclip-recorder/src/capture/dxgi/types.rs)
- The capture model now has room for GPU-backed frames in addition to CPU BGRA payloads.
  - Evidence: [src/capture/mod.rs](c:/Coding/liteclip-recorder/src/capture/mod.rs)
- A D3D11 hardware-frame encoder scaffold exists in the FFmpeg encoder.
  - Evidence: [src/encode/ffmpeg_encoder.rs](c:/Coding/liteclip-recorder/src/encode/ffmpeg_encoder.rs)

### What was validated at runtime

- AMD AMF rejects BGRA-backed D3D11 hardware input frames.
- The exact runtime failure is: `Format of input frames context (bgra) is not supported by AMF.`

This means a true zero-copy AMD path cannot stop at “D3D11 texture handoff”. It must hand off NV12 D3D11 hardware frames, not BGRA D3D11 hardware frames.

## Scope

### In scope

- Windows-only video path
- AMD AMF first
- D3D11-based zero-copy handoff for video
- GPU BGRA -> NV12 conversion
- startup-time capability selection and fallback
- preserving the current CPU path for unsupported cases

### Out of scope for the first slice

- QSV zero-copy
- NVENC zero-copy beyond existing behavior
- audio pipeline changes
- muxer changes
- replacing all fallback CPU code
- generalized multi-backend GPU abstraction

## Architecture Direction

### Target steady-state AMD path

1. Acquire desktop frame via DXGI.
2. If output resolution differs, render to a BGRA scale texture on the GPU.
3. Convert the BGRA source texture to an NV12 destination texture on the GPU.
4. Copy or render into an FFmpeg-owned D3D11 hardware frame.
5. Submit `AV_PIX_FMT_D3D11` frames with `sw_format = NV12` to AMF.
6. Drain encoded packets as today.

### Required fallback behavior

- If NV12 hardware-frame initialization fails, stay on the existing CPU path for the entire session.
- Do not attempt mid-session mode switching in the first slice.
- Keep software encoders and non-AMF encoders on CPU-readable frames.

## Work Plan

### Phase 1: Stabilize the GPU frame model

1. Keep the dual frame representation in [src/capture/mod.rs](c:/Coding/liteclip-recorder/src/capture/mod.rs): CPU BGRA and GPU D3D11 texture.
2. Ensure every GPU frame handed to the encoder is backed by an app-owned texture, never the duplication-owned texture.
3. Preserve repeat-last-frame behavior for GPU payloads by cloning texture ownership, not bytes.

Definition of done:

- capture can emit owned D3D11 textures safely
- timeout/repeat logic works for GPU payloads
- CPU fallback remains unchanged

### Phase 2: Add GPU NV12 conversion in capture

1. Add NV12 render/convert resources to [src/capture/dxgi/types.rs](c:/Coding/liteclip-recorder/src/capture/dxgi/types.rs).
2. Introduce a conversion target texture with `DXGI_FORMAT_NV12`.
3. Implement GPU conversion from BGRA to NV12.

There are two viable approaches:

- Pixel shader path with separate luma/chroma rendering strategy.
- Video Processor path using D3D11 video processing APIs.

Preferred first step:

- Start with D3D11 Video Processor if AMD drivers accept the output path cleanly.
- Fall back to shader-based conversion only if the video processor route becomes impractical.

Definition of done:

- capture can produce an NV12 D3D11 texture at the target output resolution
- no staging-texture map is needed in the AMF session path

### Phase 3: Finish FFmpeg D3D11 hardware-frame ingestion

1. Complete the D3D11 hardware context path in [src/encode/ffmpeg_encoder.rs](c:/Coding/liteclip-recorder/src/encode/ffmpeg_encoder.rs).
2. Configure `AVHWFramesContext` with:
   - `format = AV_PIX_FMT_D3D11`
   - `sw_format = AV_PIX_FMT_NV12`
   - output width and height matching encoder resolution
3. Allocate FFmpeg-owned D3D11 hardware frames.
4. Copy the capture-produced NV12 texture into those hardware frames entirely on the GPU.
5. Submit the hardware frames directly to AMF.

Definition of done:

- AMF accepts the D3D11 frames at runtime
- FFmpeg no longer initializes `swscale` for the AMF zero-copy path
- encoding succeeds without CPU BGRA frame copies

### Phase 4: Startup mode selection

1. Resolve the effective encoder first.
2. Enable GPU transport only when all of these are true:
   - platform is Windows
   - effective encoder is AMF
   - D3D11 hardware frame path initializes successfully
   - NV12 GPU conversion path initializes successfully
3. Otherwise fall back to CPU readback for the session.

Definition of done:

- the recorder never enters a half-enabled broken state
- unsupported systems still record correctly

### Phase 5: Validation and measurement

1. Compare `cargo run --release` CPU path versus NV12 GPU path.
2. Capture at 2560x1440 -> 1920x1080 60 FPS.
3. Record metrics for:
   - steady-state FPS
   - dropped frames
   - encoder errors
   - CPU utilization
   - GPU utilization
4. Validate saved clips for:
   - correct playback
   - correct keyframe cadence
   - no color corruption
   - no chroma plane misalignment

Definition of done:

- AMF path records correctly
- FPS improves versus the current CPU-conversion path
- clip outputs remain valid

## File-Level Changes

### [src/capture/mod.rs](c:/Coding/liteclip-recorder/src/capture/mod.rs)

- Keep `CapturedFrame` as the transport envelope.
- If needed, refine the GPU payload metadata so the encoder can distinguish BGRA GPU textures from NV12 GPU textures without probing texture descriptions every frame.

### [src/capture/dxgi/types.rs](c:/Coding/liteclip-recorder/src/capture/dxgi/types.rs)

- Add NV12 conversion resources.
- Add a GPU conversion pass from BGRA to NV12.
- Emit GPU-backed NV12 frames for supported sessions.
- Keep staging readback for CPU sessions.

### [src/encode/ffmpeg_encoder.rs](c:/Coding/liteclip-recorder/src/encode/ffmpeg_encoder.rs)

- Keep the current CPU path untouched.
- Finish the D3D11 hardware-frame path.
- Switch the AMF zero-copy path from `sw_format = BGRA` to `sw_format = NV12`.
- Remove CPU frame copies and `swscale` from the AMF zero-copy path.

### [src/app.rs](c:/Coding/liteclip-recorder/src/app.rs)

- Keep startup-time transport selection.
- Only disable CPU readback when the GPU transport is actually supported and initialized.

## Risks

### 1. D3D11 NV12 conversion complexity

Risk:

- NV12 conversion is the missing hard part, not the FFmpeg plumbing.

Mitigation:

- implement and validate conversion separately before forcing activation
- keep the AMF GPU path gated behind successful initialization

### 2. AMF format constraints vary by driver

Risk:

- AMD drivers may differ on accepted D3D11 frame formats or usage flags.

Mitigation:

- keep initialization strict and fail closed back to CPU readback
- log exact FFmpeg/AMF open errors

### 3. Shared D3D11 immediate context across threads

Risk:

- copy operations between capture and encoder threads can race if not protected

Mitigation:

- keep multithread protection enabled
- prefer simple GPU-to-GPU copy semantics in the first slice

### 4. False zero-copy claims

Risk:

- it is easy to keep one hidden CPU conversion in the path and think the job is done

Mitigation:

- explicitly verify that no staging `Map` and no FFmpeg `swscale` occur in the AMF path

## Milestones

### Milestone 1

- GPU texture transport exists end-to-end behind a gate
- recorder remains stable on CPU fallback

Status: mostly in place

### Milestone 2

- GPU BGRA -> NV12 conversion implemented and validated in isolation

Status: not started

### Milestone 3

- FFmpeg AMF accepts D3D11 NV12 hardware frames and records correctly

Status: not started

### Milestone 4

- zero-copy AMF path enabled by default when supported

Status: not started

## Definition of Done

- AMD HEVC path records without CPU staging readback
- AMD HEVC path records without CPU `swscale`
- saved clips are valid and seekable
- fallback CPU path still works on unsupported systems
- release build shows measurable FPS improvement over the current path

## Immediate Next Step

Implement GPU BGRA -> NV12 conversion in [src/capture/dxgi/types.rs](c:/Coding/liteclip-recorder/src/capture/dxgi/types.rs), then switch the AMF hardware-frame context in [src/encode/ffmpeg_encoder.rs](c:/Coding/liteclip-recorder/src/encode/ffmpeg_encoder.rs) from `sw_format = BGRA` to `sw_format = NV12`.