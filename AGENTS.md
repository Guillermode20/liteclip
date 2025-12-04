# Project Context

## Purpose
liteclip is a small, cross-platform desktop application that makes it fast and easy to compress and trim videos for sharing on platforms that enforce strict file size limits (e.g., Discord, WhatsApp, email). It operates entirely locally, using a native window for UI and an embedded ASP.NET Core server for media-handling APIs.

## Tech Stack
- Backend: .NET 10 (net10.0), ASP.NET Core minimal APIs, Kestrel web server
- Desktop shell: Photino.NET (native cross-platform window hosting an embedded UI)
- Video tooling: FFmpeg (managed via Xabe.FFmpeg.Downloader and bundled runtime), local ffmpeg bootstrapper service
- Frontend: Svelte 5, TypeScript, Vite
- Build: .NET SDK, npm for frontend; the csproj contains a `BuildFrontend` target to compile the UI into `wwwroot` during `dotnet build` and `dotnet publish`
- Packaging: single-file Release publish with optional embedded static assets (Release builds embed `wwwroot`)

## Project Structure (key files/folders)
- `Program.cs` - app startup: DI registration, server routing, Photino window host with WebView2 management
- `Endpoints/` - API route handlers: `CompressionEndpoints`, `SettingsEndpoints`, `SystemEndpoints`
- `Services/` - core application services:
  - FFmpeg management: `FfmpegBootstrapper`, `FfmpegPathResolver`, `FfmpegProcessRunner`, `FfmpegProbeService`, `FfmpegEncoderProbe`
  - Compression pipeline: `VideoCompressionService`, `VideoEncodingPipeline`, `DefaultCompressionPlanner`
  - Encoding: `EncoderSelectionService`, `AdaptiveFilterBuilder`, `FfmpegProgressParser`
  - Storage & cleanup: `InMemoryJobStore`, `JobCleanupService`, `UserSettingsStore`
  - Metadata: `VideoMetadataService`, `FfmpegCapabilityProbe`
  - Utilities: `UpdateCheckerService`, `AppVersionProvider`, `LoggingHelpers`
- `CompressionStrategies/` - encoding strategy implementations: `H264Strategy`, `H265Strategy`, `CompressionStrategyFactory`, and `ICompressionStrategy` interface
- `Models/` - shared DTOs: `CompressionRequest`, `CompressionResult`, `CompressionJob`, `CompressionPlan`, `UserSettings`, `JobStatus`, `FfmpegEncoderInfo`, `GitHubRelease`
- `Serialization/` - JSON serialization context for source generation
- `frontend/` - Svelte 5 app with TypeScript: `App.svelte`, component library, Vite build pipeline
- `liteclip.Tests/` - xUnit test suite with strategy tests and integration tests

## Architecture Patterns
- **Single-process desktop app**: ASP.NET Core Kestrel server (auto-assigned port 0 for conflict avoidance) hosted in-process with a Photino window via WebView2. The frontend communicates with the local API endpoints for all operations.
- **Strategy pattern**: encoding strategies (`ICompressionStrategy`) implement codec-specific parameters and bitrate calculations; `CompressionStrategyFactory` selects the appropriate strategy (H264 or H265) based on encoding mode.
- **Compression pipeline**: `VideoEncodingPipeline` orchestrates the encoding flow; `DefaultCompressionPlanner` normalizes user inputs and calculates bitrate targets; `VideoCompressionService` manages job lifecycle and concurrency.
- **DI & Service lifecycle**: all services registered via dependency injection with singleton or hosted service lifecycles. Critical services: FFmpeg bootstrapper (eager initialization), compression service (with semaphore-based concurrency control), job cleanup (background timer).
- **Job store abstraction**: `IJobStore` (currently `InMemoryJobStore`) tracks compression jobs with progress, status, and results. Extensible for future persistence layers.
- **FFmpeg management**: `FfmpegBootstrapper` ensures FFmpeg availability at startup; `FfmpegPathResolver` locates the executable; `FfmpegProcessRunner` executes encoding with real-time progress parsing via `FfmpegProgressParser`.
- **Encoder capability detection**: `FfmpegEncoderProbe` and `EncoderSelectionService` detect hardware acceleration availability (nvenc, hevc_nvenc, qsv, etc.) and log encoder capabilities for diagnostic purposes.
- **File handling/limits**: Kestrel configured for 2GB uploads; form options similarly set to 2GB; endpoints validate file size, duration, and request parameters before processing.
- **Window lifecycle**: WebView2 user data folder managed with per-instance temp directories; old profiles cleaned up after 1 day. Window starts off-screen to prevent white flash, shows on "window-ready" message from frontend.

## Conventions
- **C# code style**: standard .NET naming conventions — PascalCase for types/methods, camelCase for private fields/local vars, `var` acceptable when type is obvious.
- **Project settings**: `nullable` reference types enabled, `ImplicitUsings` enabled for cleaner code. Enable `#nullable enable` for files that need strict null checking.
- **Dependency injection**: all services constructor-injected. Register services in `Program.cs` `ConfigureServices()` method.
- **Logging**: use `ILogger<T>` via DI. Development builds use `LogLevel.Trace`; production uses `LogLevel.Information`. Use `LoggingHelpers` for structured logging patterns.
- **Error handling**: minimal APIs return typed `Results` (Ok, BadRequest, NotFound, Problem) with consistent HTTP status codes. Include detailed error messages in Problem responses for debugging.
- **Frontend**: Svelte 5 + TypeScript in `frontend/` folder. `App.svelte` is root component. Run `npm run check` (svelte-check + TypeScript) before commits. All UI state and API calls typed via TypeScript.
- **API requests**: frontend uses fetch to `http://127.0.0.1:<dynamic-port>` determined at startup. Compression uploads use FormData with multipart/form-data.
- **Configuration**: use `appsettings.json` for static config; runtime config via environment or code (e.g., temp directory paths from LocalApplicationData).

## Testing Strategy
- Frontend: use `npm run check` (svelte-check + TypeScript checks) and add component/unit tests (e.g., Vitest or Playwright for integration) as needed.
- Backend: add xUnit or similar test projects for critical business logic (Compression strategies, bitrate math, request validation). Mock external dependencies (FFmpeg related calls) using abstractions (e.g., `IFfmpegPathResolver`) for reliable tests.
- Integration: add a CI step that runs `dotnet build`, `npm ci && npm run build`, and backend tests to ensure the release artifact builds cleanly.

## Git Workflow & Commit Conventions
- Branching: use `main` for releases; create feature branches `feature/xxx` for larger changes and `fix/xxx` for bug fixes.
- Pull requests: include a brief description, list of files changed, testing notes, and link to issues when applicable.
- Commits: follow Conventional Commits style (e.g., `feat: add h265 strategy`, `fix: handle null segments in request`) for consistent changelogs and release notes.

## Domain Context
- The main domain is video compression with a focus on preserving visual quality while achieving a target file size.
- The app operates offline and purely locally: no video or metadata should be uploaded to third-party servers.
- FFmpeg is used as the encoder; the project supports H.264 & H.265 strategies. Hardware acceleration detection is implemented in service code (logging and encoder selection).

## Important Constraints
- **Offline by design**: no video or metadata uploaded to external servers. Compression is entirely local. Update checks are the only network feature and must fail gracefully.
- **Max upload size**: Kestrel and form options configured for 2GB (`MultipartBodyLengthLimit = 2_147_483_648`). Validate file size in endpoints before processing.
- **Release packaging**: Release builds are single-file self-contained executables with embedded frontend assets. Debug builds use physical `wwwroot/` files copied to output directory.
- **Window lifecycle**: WebView2 temp directories must be isolated per instance to prevent locking. Profiles older than 1 day are auto-cleaned to prevent disk bloat.
- **FFmpeg bundling**: FFmpeg is downloaded at runtime via `Xabe.FFmpeg.Downloader`. `FfmpegBootstrapper` runs on startup (eager initialization). Tests must mock FFmpeg calls or use isolation.
- **Port assignment**: Kestrel uses port 0 (OS auto-assigns) to avoid conflicts. Frontend discovers the port from server startup output or environment.

## External Dependencies
- `Photino.NET`: native windowing host for the frontend
- `Xabe.FFmpeg.Downloader`: helps download and manage the ffmpeg executable at runtime
- Vite + Svelte + TypeScript: frontend stack
- `Microsoft.Extensions` libraries in .NET for DI, logging, hosting and configuration

## Contribution Guidance
- Read `build.md` before contributing to understand build/publish steps.
- Keep small, focused PRs with documented testing steps.
- Run `npm run check` in `frontend/` and `dotnet build` before opening a PR.
- If adding a new encoding strategy, implement `ICompressionStrategy`, register it with DI, and add unit tests validating the bitrate estimation and parameter transformation.
