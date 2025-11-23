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
- `Program.cs` - app startup: DI registration, server routing, Photino window host
- `Services/` - application services (FFmpeg bootstrapper, path resolver, compression service, update checker, job cleanup)
- `CompressionStrategies/` - strategy implementations: H264/H265, factory, and `ICompressionStrategy` interface
- `Models/` - shared DTOs: `CompressionRequest`, `CompressionResult`, `UserSettings`
- `frontend/` - Svelte app and public assets

## Architecture Patterns
- Single-process desktop app: an ASP.NET Core server and a native window hosted in the same process using Photino. The front-end talks to the local server endpoints for compression operations.
- Strategy pattern: encoding strategies (`ICompressionStrategy`) implement specific encoders/parameters; `CompressionStrategyFactory` selects the appropriate strategy.
- DI & Hosted Services: all central services (VideoCompressionService, Ffmpeg path resolver/bootstrapper, and JobCleanup) are registered through DI and use hosted or singleton lifecycles where appropriate.
- Background processing: `VideoCompressionService` processes compression jobs in the background; `JobCleanupService` periodically cleans stale jobs/files.
- File handling/limits: the Kestrel server is configured for large uploads (configured up to ~2GB), and endpoints perform validation and error handling for uploads and downloads.

## Conventions
- C# code style: use standard .NET naming conventions — PascalCase for types/methods, camelCase for private fields/local vars, `var` is acceptable when the type is obvious.
- Enable `nullable` reference types (project default) and `ImplicitUsings` for cleaner code.
- Use DI for services; constructor-inject dependencies in services and controllers.
- Keep UI code in `frontend/` (Svelte): `App.svelte` is the root; use TypeScript for critical logic. Run `npm run check` to lint and type-check frontend code.
- Logging: use `ILogger<T>` and `builder.Logging.SetMinimumLevel(LogLevel.Trace)` for server-side debug during development.
- Error handling: minimal APIs return typed Problem responses and consistent status codes. Follow existing patterns for returning `Results.Ok`, `Results.BadRequest`, `Results.NotFound`, and `Results.Problem` for errors.

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
- Offline by design: do not rely on remote services for video processing. Network-only features such as update checks explicitly call an update checker, but compression is local.
- Max upload size: the server is configured to accept up to ~2GB by default; this can change via configuration (`FileUpload:MaxFileSizeBytes`).
- Release packaging: Release builds embed the frontend into the final single-file binary; keep `wwwroot` small and optimized.
- FFmpeg: the app downloads or bundles FFmpeg using `Xabe.FFmpeg.Downloader`; tests that depend on FFmpeg should mock or isolate this dependency.

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

## Notes & Next Steps
- If you want, I can also add a basic `dotnet test` project, a `CONTRIBUTING.md`, and a CI GitHub Actions file to validate builds and run checks on PRs.
