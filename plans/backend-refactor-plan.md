# Backend Refactor TODOs (LiteClip)

Rough, high-level checklist to gradually de-fragment the backend. Tweak ordering as needed.

---

## Phase 1 – Program.cs & Endpoints

- [ ] Extract compression-related endpoints into `Endpoints/CompressionEndpoints.cs`
  - [ ] Map `/api/compress`
  - [ ] Map `/api/status/{jobId}`
  - [ ] Map `/api/download/{jobId}`
- [ ] Extract settings & FFmpeg status endpoints into dedicated endpoint modules
  - [ ] `Endpoints/SettingsEndpoints.cs` for `/api/settings`
  - [ ] `Endpoints/SystemEndpoints.cs` (or similar) for `/api/ffmpeg/status` and `/api/update`
- [ ] Keep `Program.cs` focused on:
  - [ ] Service registration (DI)
  - [ ] Building the `WebApplication`
  - [ ] Mapping endpoint groups via extension methods
  - [ ] Photino window + Kestrel startup wiring only

---

## Phase 2 – Decompose VideoCompressionService

- [ ] Introduce a `CompressionPlan` model that captures:
  - [ ] Target size / bitrate budget
  - [ ] Selected codec & encoder
  - [ ] Scale / FPS decisions
  - [ ] Filter chain / quality settings
  - [ ] Normalized segments
- [ ] Extract a planning component (e.g., `ICompressionPlanner`)
  - [ ] Move `CalculateBitratePlan` logic
  - [ ] Move `CalculateOptimalScale` and related resolution logic
  - [ ] Centralize segment normalization and validation
- [ ] Extract a job store / lifecycle abstraction (e.g., `IJobStore`)
  - [ ] Own `_jobs` dictionary and `_jobQueue`
  - [ ] Provide `CreateJob`, `UpdateJob`, `GetJob`, `GetAllJobs`, `Enqueue`, etc.
  - [ ] Hide concurrent access details from the rest of the code
- [ ] Slim `VideoCompressionService` down to orchestration:
  - [ ] Validate input
  - [ ] Ask `ICompressionPlanner` for a `CompressionPlan`
  - [ ] Use `IJobStore` to create/enqueue jobs
  - [ ] Delegate execution to an FFmpeg executor (next phase)

---

## Phase 3 – FFmpeg Execution & Progress Parsing

- [ ] Introduce an `IFfmpegRunner` / `FfmpegProcessRunner`
  - [ ] Centralize `ProcessStartInfo` construction for both passes
  - [ ] Handle cancellation, timeouts, and error codes in one place
  - [ ] Return a result object (exit code, logs, output path)
- [ ] Extract stderr progress parsing into a dedicated component
  - [ ] `IProgressParser` that parses `time=`, `bitrate=`, `speed=`, etc.
  - [ ] Calculate ETA and percent complete based on total duration
  - [ ] Keep job mutation (updating `job.Progress`, ETA) outside parser
- [ ] Make `RunTwoPassEncodingAsync` just:
  - [ ] Build arguments for pass 1 & 2
  - [ ] Call `IFfmpegRunner` with a progress callback
  - [ ] Update `CompressionJob` state from progress events

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
