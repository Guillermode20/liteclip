# Backend Refactor TODOs (LiteClip)

Rough, high-level checklist to gradually de-fragment the backend. Tweak ordering as needed.

---

## Phase 1 – Program.cs & Endpoints

- [x] Extract compression-related endpoints into `Endpoints/CompressionEndpoints.cs`
  - [x] Map `/api/compress`
  - [x] Map `/api/status/{jobId}`
  - [x] Map `/api/download/{jobId}`
- [x] Extract settings & FFmpeg status endpoints into dedicated endpoint modules
  - [x] `Endpoints/SettingsEndpoints.cs` for `/api/settings`
  - [x] `Endpoints/SystemEndpoints.cs` (or similar) for `/api/ffmpeg/status` and `/api/update`
- [x] Keep `Program.cs` focused on:
  - [x] Service registration (DI)
  - [x] Building the `WebApplication`
  - [x] Mapping endpoint groups via extension methods
  - [x] Photino window + Kestrel startup wiring only

---

## Phase 2 – Decompose VideoCompressionService

- [x] Introduce a `CompressionPlan` model that captures:
  - [x] Target size / bitrate budget
  - [ ] Selected codec & encoder
  - [ ] Scale / FPS decisions
  - [ ] Filter chain / quality settings
  - [x] Normalized segments
- [x] Extract a planning component (e.g., `ICompressionPlanner`)
  - [x] Move `CalculateBitratePlan` logic
  - [x] Move `CalculateOptimalScale` and related resolution logic
  - [x] Centralize segment normalization and validation
- [x] Extract a job store / lifecycle abstraction (e.g., `IJobStore`)
  - [x] Own `_jobs` dictionary and `_jobQueue`
  - [x] Provide `CreateJob`, `UpdateJob`, `GetJob`, `GetAllJobs`, `Enqueue`, etc.
  - [x] Hide concurrent access details from the rest of the code
- [x] Slim `VideoCompressionService` down to orchestration:
  - [x] Validate input
  - [x] Ask `ICompressionPlanner` for a `CompressionPlan`
  - [x] Use `IJobStore` to create/enqueue jobs
  - [ ] Delegate execution to an FFmpeg executor (next phase)

---

## Phase 3 – FFmpeg Execution & Progress Parsing

- [x] Introduce an `IFfmpegRunner` / `FfmpegProcessRunner`
  - [x] Centralize `ProcessStartInfo` construction for both passes
  - [x] Handle cancellation, timeouts, and error codes in one place
  - [x] Return a result object (exit code, logs, output path)
- [x] Extract stderr progress parsing into a dedicated component
  - [x] `IProgressParser` that parses `time=`, `bitrate=`, `speed=`, etc.
  - [x] Calculate ETA and percent complete based on total duration
  - [x] Keep job mutation (updating `job.Progress`, ETA) outside parser
- [x] Make `RunTwoPassEncodingAsync` just:
  - [x] Build arguments for pass 1 & 2
  - [x] Call `IFfmpegRunner` with a progress callback
  - [x] Update `CompressionJob` state from progress events

---

## Phase 4 – Hardware Encoder Detection & Strategy Cleanup

- [ ] Centralize hardware encoder probing
  - [ ] Move encoder availability checks out of `BaseCompressionStrategy`
  - [ ] Use/extend `FfmpegCapabilityProbe` (or create `IFfmpegEncoderProbe`)
  - [ ] Cache probe results for the process lifetime
- [ ] Create an `IEncoderSelectionService`
  - [ ] Encapsulate policy: NVENC → QSV → VideoToolbox → AMF → software
  - [ ] Strategy classes ask this service for the best encoder per codec
- [ ] Keep strategies focused on argument construction
  - [ ] `BuildVideoArgs` uses `EncodingModeConfigs` + `CompressionPlan`
  - [ ] No direct process spawning or probing in strategies

---

## Phase 5 – Validation & API Consistency

- [ ] Introduce a `ICompressionRequestValidator`
  - [ ] Validate file presence, type, and configured max size
  - [ ] Validate/normalize segments JSON
  - [ ] Validate target size, duration, and mode
- [ ] Standardize error responses
  - [ ] Use a small helper or factory for `ProblemDetails`-style responses
  - [ ] Ensure `/api/compress`, `/api/settings`, etc. return consistent shapes
- [ ] Add focused tests around validation behavior

---

## Phase 6 – Configuration & Options

- [ ] Introduce `Options` classes for key tunables
  - [ ] `FileUploadOptions` (e.g., max file size)
  - [ ] `CompressionOptions` (default modes, safety limits)
  - [ ] `CleanupOptions` (retention durations, intervals)
- [ ] Wire them up via configuration binding
  - [ ] `builder.Services.Configure<T>(builder.Configuration.GetSection(...))`
  - [ ] Inject via `IOptions<T>` / `IOptionsMonitor<T>` instead of reading config ad hoc

---

## Phase 7 – Job Cleanup & File Management

- [ ] Move job cleanup responsibilities into dedicated abstractions
  - [ ] `IJobLifecycleManager` (may be same as `IJobStore` or layered on top)
  - [ ] `IJobFileManager` to own file deletion (input/output paths)
- [ ] Update `JobCleanupService` to depend on lifecycle APIs
  - [ ] Use `GetAllJobs()` + `ShouldCleanupJob` policy
  - [ ] Call into `IJobLifecycleManager.CleanupJob(jobId)`
- [ ] Ensure cleanup behavior is well-logged and covered with tests

---

## Phase 8 – Nice-to-Haves / Polishing

- [ ] Add tests around `CompressionPlan` creation and edge cases
- [ ] Add tests for progress parsing on representative FFmpeg stderr samples
- [ ] Document the compression pipeline (request → plan → job → FFmpeg → cleanup)
- [ ] Consider small refactors on the frontend to align naming with new backend concepts (e.g., `CompressionPlan`, job statuses)
