# LiteClip Technical Specification for Avalonia Migration

This document provides a comprehensive technical description of the current LiteClip application architecture to serve as a migration plan from **Photino.NET + ASP.NET Core backend** to a **fully local Avalonia application**.

---

## Table of Contents

1. [Application Overview](#1-application-overview)
2. [Current Architecture](#2-current-architecture)
3. [Core Domain Logic](#3-core-domain-logic)
4. [Data Models](#4-data-models)
5. [Services Layer](#5-services-layer)
6. [API Endpoints (Current HTTP Interface)](#6-api-endpoints-current-http-interface)
7. [Frontend Components & State](#7-frontend-components--state)
8. [User Settings & Persistence](#8-user-settings--persistence)
9. [FFmpeg Integration](#9-ffmpeg-integration)
10. [Avalonia Migration Strategy](#10-avalonia-migration-strategy)
11. [Component Mapping](#11-component-mapping)
12. [Migration Checklist](#12-migration-checklist)

---

## 1. Application Overview

### Purpose
LiteClip is a cross-platform desktop application for fast, local video compression and trimming. It targets users who need to compress videos for platforms with strict file size limits (Discord, WhatsApp, email).

### Key Features
- **Video compression** with target file size (bitrate-based encoding)
- **Video trimming/editing** with segment selection
- **Two encoding modes**: Fast (H.264) and Quality (H.265)
- **Hardware acceleration** detection (NVENC, QSV, AMF)
- **Audio muting** option
- **Resolution scaling** (auto-calculated or preset)
- **Progress tracking** with ETA estimation
- **Job queue** with concurrency control
- **User settings** persistence
- **Update checking** (GitHub releases)
- **100% local processing** - no cloud uploads

### Current Tech Stack
| Layer | Technology |
|-------|------------|
| Desktop Shell | Photino.NET (WebView2 wrapper) |
| Backend | ASP.NET Core Minimal APIs + Kestrel |
| Frontend | Svelte 5 + TypeScript + Vite |
| Video Processing | FFmpeg (via process execution) |
| FFmpeg Management | Xabe.FFmpeg.Downloader |
| Build | .NET 10, npm |

---

## 2. Current Architecture

### Process Model
```
┌─────────────────────────────────────────────────────────────┐
│                    Single .NET Process                       │
│  ┌─────────────────┐    ┌─────────────────────────────────┐ │
│  │  Photino Window │◄──►│  ASP.NET Core Kestrel Server    │ │
│  │   (WebView2)    │    │  (http://127.0.0.1:<dynamic>)   │ │
│  │                 │    │                                  │ │
│  │  ┌───────────┐  │    │  ┌─────────────────────────────┐│ │
│  │  │  Svelte   │  │HTTP│  │  Minimal API Endpoints      ││ │
│  │  │  Frontend │──┼────┼──│  /api/compress              ││ │
│  │  │           │  │    │  │  /api/status/{jobId}        ││ │
│  │  └───────────┘  │    │  │  /api/download/{jobId}      ││ │
│  └─────────────────┘    │  │  /api/settings              ││ │
│                         │  │  /api/ffmpeg/status         ││ │
│                         │  └─────────────────────────────┘│ │
│                         │                                  │ │
│                         │  ┌─────────────────────────────┐│ │
│                         │  │  Services (DI Container)    ││ │
│                         │  │  - VideoCompressionService  ││ │
│                         │  │  - FfmpegBootstrapper       ││ │
│                         │  │  - EncoderSelectionService  ││ │
│                         │  │  - UserSettingsStore        ││ │
│                         │  └─────────────────────────────┘│ │
│                         └─────────────────────────────────┘ │
└─────────────────────────────────────────────────────────────┘
                              │
                              ▼
                    ┌─────────────────┐
                    │  FFmpeg Process │
                    │  (child process)│
                    └─────────────────┘
```

### Startup Flow
1. `Main()` runs on `[STAThread]` (required for WebView2)
2. `WebApplicationBuilder` configures DI and services
3. Kestrel starts on `http://127.0.0.1:0` (OS-assigned port)
4. `FfmpegBootstrapper.PrimeExistingInstallation()` checks for FFmpeg
5. `PhotinoWindow` created, positioned off-screen to prevent white flash
6. Window loads server URL, frontend sends `window-ready` message
7. Window moves on-screen and becomes visible
8. `window.WaitForClose()` blocks main thread (message pump)
9. On close: cancel jobs, stop server, exit

### Communication Pattern
- **Frontend → Backend**: HTTP fetch to `http://127.0.0.1:<port>/api/*`
- **Backend → Frontend**: Polling (frontend polls `/api/status/{jobId}` every 500ms)
- **Window Messages**: `window.external.sendMessage()` for close-app, window-ready

---

## 3. Core Domain Logic

### Compression Pipeline

```
┌──────────────┐    ┌──────────────────┐    ┌─────────────────┐
│ Upload Video │───►│ Normalize Request│───►│ Build Comp Plan │
└──────────────┘    └──────────────────┘    └─────────────────┘
                                                     │
                    ┌────────────────────────────────┘
                    ▼
┌──────────────────────────────────────────────────────────────┐
│                    Compression Decision                       │
│  ┌─────────────────────────────────────────────────────────┐ │
│  │ Skip Compression if:                                     │ │
│  │   - skipCompression flag set                             │ │
│  │   - targetSizeMb >= effectiveMaxSize                     │ │
│  │   - (unless muteAudio is true)                           │ │
│  └─────────────────────────────────────────────────────────┘ │
└──────────────────────────────────────────────────────────────┘
                    │
        ┌───────────┴───────────┐
        ▼                       ▼
┌───────────────┐       ┌───────────────────┐
│ Skip: Copy    │       │ Queue for FFmpeg  │
│ File Directly │       │ Processing        │
└───────────────┘       └───────────────────┘
                                │
                    ┌───────────┴───────────┐
                    ▼                       ▼
            ┌───────────────┐       ┌───────────────┐
            │ Single-Pass   │       │ Two-Pass      │
            │ (Hardware)    │       │ (Software)    │
            └───────────────┘       └───────────────┘
                    │                       │
                    └───────────┬───────────┘
                                ▼
                    ┌───────────────────────┐
                    │ FFmpeg Process Runner │
                    │ - Build arguments     │
                    │ - Execute process     │
                    │ - Parse progress      │
                    │ - Report ETA          │
                    └───────────────────────┘
                                │
                                ▼
                    ┌───────────────────────┐
                    │ Job Completed/Failed  │
                    │ - Update job store    │
                    │ - Cleanup temp files  │
                    └───────────────────────┘
```

### Bitrate Calculation Logic
Located in `DefaultCompressionPlanner.cs`:

```csharp
// Target size with 10% safety margin
var targetSizeMb = request.TargetSizeMb.Value * 0.90;

// Reserve budget for container overhead
var reserveBudgetMb = CalculateReserveBudget(targetSizeMb, durationSeconds, codecContext);

// Calculate payload budget
var payloadBudgetMb = targetSizeMb - reserveBudgetMb;
var payloadBits = payloadBudgetMb * 1024 * 1024 * 8;
var totalKbps = payloadBits / durationSeconds / 1000;

// Subtract audio budget
var audioBudgetKbps = request.MuteAudio ? 0 : codecContext.AudioBitrateKbps * 0.9;
var videoKbps = Math.Max(80, totalKbps - audioBudgetKbps);
```

### Adaptive Resolution Scaling
Auto-calculates optimal scale based on bitrate budget:

```csharp
// Target bits-per-pixel by codec
double targetBpp = codec switch {
    "h265" => 0.065,
    "h264" => 0.095,
    _ => 0.095
};

var maxPixels = (videoKbps * 1000) / (fps * targetBpp);
var scale = Math.Sqrt(maxPixels / currentPixels);
var percent = Math.Clamp(((int)(scale * 100) + 4) / 5 * 5, 25, 100);
```

### Segment Processing
For video trimming, segments are processed via FFmpeg filters or input seeking:

```csharp
// Single segment optimization: use input seeking
// -ss <start> -t <duration> -i <input>
// This avoids decoding pre-trim content

// Multiple segments: use filter-based approach
// -filter_complex "select='between(t,0,5)+between(t,10,15)',setpts=N/FRAME_RATE/TB"
```

---

## 4. Data Models

### CompressionRequest
```csharp
public class CompressionRequest
{
    public string Codec { get; set; } = "h264";        // "h264" or "h265"
    public int? ScalePercent { get; set; }              // 10-100
    public int? TargetFps { get; set; }                 // 1-240, default 30
    public double? TargetSizeMb { get; set; }           // Target output size
    public bool SkipCompression { get; set; }           // Copy without re-encoding
    public bool MuteAudio { get; set; }                 // Remove audio track
    public double? SourceDuration { get; set; }         // Video duration in seconds
    public List<VideoSegment>? Segments { get; set; }   // Trim segments
    public bool UseQualityMode { get; set; }            // Quality vs Fast mode
    public EncodingMode Mode { get; set; }              // Derived from UseQualityMode
}

public class VideoSegment
{
    public double Start { get; set; }  // Start time in seconds
    public double End { get; set; }    // End time in seconds
}
```

### CompressionJob (Internal State)
```csharp
public class CompressionJob
{
    public string JobId { get; set; }
    public string OriginalFilename { get; set; }
    public string Codec { get; set; }
    public int? ScalePercent { get; set; }
    public double? TargetSizeMb { get; set; }
    public double? TargetBitrateKbps { get; set; }
    public string? OutputPath { get; set; }
    public string? OutputFilename { get; set; }
    public string? OutputMimeType { get; set; }
    public double Progress { get; set; }              // 0.0 - 100.0
    public int? EstimatedSecondsRemaining { get; set; }
    public int? QueuePosition { get; set; }
    public string? ErrorMessage { get; set; }
    public JobStatus Status { get; set; }             // Queued/Processing/Completed/Failed/Cancelled
    public double? SourceDuration { get; set; }
}
```

### CompressionResult (API Response)
```csharp
public class CompressionResult
{
    public string JobId { get; set; }
    public string OriginalFilename { get; set; }
    public string Status { get; set; }
    public string? Message { get; set; }
    public string Codec { get; set; }
    public int? ScalePercent { get; set; }
    public double? TargetSizeMb { get; set; }
    public double? TargetBitrateKbps { get; set; }
    public string? OutputFilename { get; set; }
    public string? OutputMimeType { get; set; }
    public long? OutputSizeBytes { get; set; }
    public bool CompressionSkipped { get; set; }
    public string? EncoderName { get; set; }
    public bool? EncoderIsHardware { get; set; }
    public double Progress { get; set; }
    public int? EstimatedSecondsRemaining { get; set; }
    public int? QueuePosition { get; set; }
    public DateTime? CreatedAt { get; set; }
    public DateTime? CompletedAt { get; set; }
}
```

### UserSettings
```csharp
public class UserSettings
{
    public string DefaultCodec { get; set; } = "quality";     // "fast" or "quality"
    public string DefaultResolution { get; set; } = "auto";   // "auto", "source", "1080p", etc.
    public bool DefaultMuteAudio { get; set; }
    public double DefaultTargetSizeMb { get; set; } = 25;
    public bool CheckForUpdatesOnLaunch { get; set; } = true;
    public bool StartMaximized { get; set; } = true;
    public string DefaultFolder { get; set; } = string.Empty;
    public double AppScale { get; set; } = 1.0;
}
```

### VideoMetadataResult
```csharp
public sealed class VideoMetadataResult
{
    public int Width { get; init; }
    public int Height { get; init; }
    public double Duration { get; init; }
    public double AspectRatio => Height > 0 ? (double)Width / Height : 0;
    public string? Codec { get; init; }
    public double? FrameRate { get; init; }
    public long? Bitrate { get; init; }
    public string? PixelFormat { get; init; }
    public bool HasAudio { get; init; }
    public string? AudioCodec { get; init; }
    public int? AudioChannels { get; init; }
    public int? AudioSampleRate { get; init; }
}
```

---

## 5. Services Layer

### Service Dependency Graph

```
┌─────────────────────────────────────────────────────────────────────┐
│                         Service Dependencies                         │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  VideoCompressionService                                             │
│    ├── InMemoryJobStore                                              │
│    ├── FfmpegPathResolver                                            │
│    ├── IFfmpegRunner (FfmpegProcessRunner)                           │
│    ├── ICompressionStrategyFactory                                   │
│    ├── DefaultCompressionPlanner                                     │
│    └── EncoderSelectionService                                       │
│           └── FfmpegEncoderProbe                                     │
│                  └── FfmpegPathResolver                              │
│                                                                      │
│  FfmpegBootstrapper                                                  │
│    └── IFfmpegPathResolver                                           │
│                                                                      │
│  VideoMetadataService                                                │
│    └── IFfmpegPathResolver                                           │
│                                                                      │
│  FfmpegProbeService                                                  │
│    └── FfmpegPathResolver                                            │
│                                                                      │
│  UserSettingsStore (standalone)                                      │
│  UpdateCheckerService (standalone, uses HttpClient)                  │
│  AppVersionProvider (standalone)                                     │
│  JobCleanupService (HostedService)                                   │
│    └── InMemoryJobStore                                              │
│                                                                      │
└─────────────────────────────────────────────────────────────────────┘
```

### Service Descriptions

#### VideoCompressionService
**Purpose**: Main orchestrator for video compression jobs.

**Responsibilities**:
- Accept upload files and compression requests
- Normalize requests via `DefaultCompressionPlanner`
- Queue jobs with concurrency control (semaphore-based, default 2 concurrent)
- Execute FFmpeg compression (single-pass or two-pass)
- Track progress and ETA
- Handle job cancellation and retry
- Manage temp file cleanup

**Key Methods**:
```csharp
Task<string> CompressVideoAsync(IFormFile videoFile, CompressionRequest request)
CompressionJob? GetJob(string jobId)
bool CancelJob(string jobId)
(bool Success, string? Error) RetryJob(string jobId)
int GetQueuePosition(string jobId)
void CancelAllJobs()
```

#### FfmpegBootstrapper
**Purpose**: Ensures FFmpeg binaries are available at runtime.

**Responsibilities**:
- Check for existing FFmpeg installation
- Download FFmpeg via `Xabe.FFmpeg.Downloader` if missing
- Report download progress
- Handle permission issues (fallback to LocalAppData)
- Grant Unix execute permissions on non-Windows

**States**: `Idle`, `Checking`, `Downloading`, `Ready`, `Error`

**Key Methods**:
```csharp
Task EnsureReadyAsync()
FfmpegBootstrapStatus GetStatus()
void PrimeExistingInstallation()  // Quick sync check at startup
void ResetForRetry()
```

#### FfmpegPathResolver
**Purpose**: Locate FFmpeg/FFprobe executables.

**Search Order**:
1. Configuration override (`FFmpeg:Path`)
2. Bundled path override (`FFmpeg:BundledPath`)
3. `<AppBase>/ffmpeg/ffmpeg.exe`
4. `<AppBase>/ffmpeg.exe`
5. Entry assembly directory
6. Process directory
7. `%LocalAppData%/LiteClip/ffmpeg/ffmpeg.exe`
8. `runtimes/<rid>/native/` directories
9. System PATH (if `FFmpeg:AllowSystemPath=true`)

#### EncoderSelectionService
**Purpose**: Select best available encoder with hardware preference.

**Hardware Preference Order**:
- H.264: `h264_nvenc` → `h264_qsv` → `h264_amf` → `libx264`
- H.265: `hevc_nvenc` → `hevc_qsv` → `hevc_amf` → `libx265`

**Caching**: Encoder selection is cached per codec to avoid repeated probing.

#### DefaultCompressionPlanner
**Purpose**: Normalize requests and calculate bitrate plans.

**Responsibilities**:
- Derive encoding mode from `UseQualityMode` flag
- Normalize and merge video segments
- Calculate target bitrate from size/duration
- Calculate optimal resolution scale
- Build compression plan with all parameters

#### InMemoryJobStore
**Purpose**: Thread-safe job storage and queue management.

**Data Structures**:
- `ConcurrentDictionary<string, JobMetadata>` for job storage
- `ConcurrentQueue<string>` for job queue

#### VideoMetadataService
**Purpose**: Extract video metadata using ffprobe.

**Probe Strategies** (in order):
1. JSON output with targeted fields
2. CSV fallback for essential fields
3. Raw output parsing (last resort)

#### UserSettingsStore
**Purpose**: Persist and retrieve user settings.

**Storage Location**: `%AppData%/LiteClip/user-settings.json`

**Features**:
- Thread-safe with semaphore
- In-memory caching
- Input sanitization

#### JobCleanupService
**Purpose**: Background service for cleaning up old jobs.

**Behavior**:
- Runs as `IHostedService`
- Periodically removes completed/failed jobs older than threshold
- Cleans up associated temp files

#### UpdateCheckerService
**Purpose**: Check GitHub releases for updates.

**Features**:
- Fetches latest release from GitHub API
- Compares versions
- Caches results to avoid repeated requests

---

## 6. API Endpoints (Current HTTP Interface)

### Compression Endpoints

| Method | Path | Description |
|--------|------|-------------|
| POST | `/api/compress` | Upload and start compression |
| GET | `/api/status/{jobId}` | Get job status and progress |
| POST | `/api/cancel/{jobId}` | Cancel a running/queued job |
| POST | `/api/retry/{jobId}` | Retry a failed/cancelled job |
| GET | `/api/download/{jobId}` | Download compressed video |

#### POST /api/compress
**Request**: `multipart/form-data`
```
file: IFormFile (required)
scalePercent: int?
codec: string? ("h264" | "h265")
targetSizeMb: double?
sourceDuration: double?
segments: string? (JSON array of {start, end})
skipCompression: bool?
qualityMode: bool?
muteAudio: bool?
```

**Response**: `CompressionResult`

#### GET /api/status/{jobId}
**Response**: `CompressionResult` with current progress

#### GET /api/download/{jobId}
**Response**: File stream with `Content-Disposition: attachment`

### Settings Endpoints

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/settings` | Get user settings |
| POST | `/api/settings` | Update user settings |

### System Endpoints

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/version` | Get app version |
| GET | `/api/update` | Check for updates |
| GET | `/api/ffmpeg/status` | Get FFmpeg bootstrap status |
| POST | `/api/ffmpeg/retry` | Retry FFmpeg download |
| POST | `/api/ffmpeg/start` | Start FFmpeg download |
| GET | `/api/ffmpeg/encoders` | List available encoders |
| POST | `/api/probe-metadata` | Probe video metadata |
| POST | `/api/app/close` | Close application |

---

## 7. Frontend Components & State

### Component Hierarchy

```
App.svelte
├── Header.svelte
│   └── Settings button, version display
├── FfmpegOverlay.svelte
│   └── FFmpeg download progress overlay
├── Sidebar.svelte
│   └── Compression settings controls
├── UploadArea.svelte
│   └── Drag-and-drop file upload
├── VideoEditor.svelte (lazy-loaded)
│   ├── EditorHeader.svelte
│   ├── VideoPreview.svelte
│   ├── Timeline.svelte
│   └── SegmentsList.svelte
├── ProgressCard.svelte
│   └── Compression progress display
├── StatusCard.svelte
│   └── Status messages
├── OutputPanel.svelte
│   └── Download and output info
└── SettingsModal.svelte (lazy-loaded)
    └── User preferences
```

### State Categories

#### File & Video Source
```typescript
let selectedFile: File | null = null;
let sourceVideoWidth: number | null = null;
let sourceVideoHeight: number | null = null;
let sourceDuration: number | null = null;
let originalSizeMb: number | null = null;
let videoSegments: VideoSegment[] = [];
```

#### Job & Processing
```typescript
let jobId: string | null = null;
let isCompressing = false;
let progressPercent = 0;
let showCancelButton = false;
let canRetry = false;
let compressionSkipped = false;
```

#### UI Visibility
```typescript
let controlsVisible = false;
let statusVisible = false;
let progressVisible = false;
let videoPreviewVisible = false;
let downloadVisible = false;
let showVideoEditor = false;
let showSettingsModal = false;
```

#### Compression Settings
```typescript
let outputSizeSliderValue = 100;
let codecSelectValue: CodecKey = 'quality';
let muteAudio = false;
let resolutionPreset: ResolutionPreset = 'auto';
```

#### Output Metadata
```typescript
interface OutputMetadata {
    outputSizeBytes: number;
    outputSizeMb: number;
    compressionRatio: number;
    targetBitrateKbps: number;
    videoBitrateKbps: number;
    scalePercent: number;
    codec: string;
    encoderName: string | null;
    encoderIsHardware: boolean;
    encodingTime: number;
    finalDuration: number;
    finalWidth: number;
    finalHeight: number;
}
```

### API Service Layer
Located in `frontend/src/services/api.ts`:

```typescript
export async function getSettings(): Promise<UserSettingsPayload>
export async function saveSettings(settings: UserSettingsPayload): Promise<UserSettingsPayload>
export async function uploadVideo(formData: FormData, signal?: AbortSignal): Promise<{ jobId: string }>
export async function getJobStatus(jobId: string): Promise<CompressionStatusResponse>
export async function cancelJob(jobId: string): Promise<void>
export async function retryJob(jobId: string): Promise<void>
export async function getFfmpegStatus(): Promise<FfmpegStatusResponse>
export async function closeApp(): Promise<void>
```

### Video Editor Features
- Video preview with playback controls
- Timeline with visual waveform/thumbnails
- Segment selection (click to cut)
- Segment list with delete capability
- Keyboard shortcuts (Space for play/pause)
- Drag-based scrubbing

---

## 8. User Settings & Persistence

### Settings Schema
```json
{
    "defaultCodec": "quality",
    "defaultResolution": "auto",
    "defaultMuteAudio": false,
    "defaultTargetSizeMb": 25,
    "checkForUpdatesOnLaunch": true,
    "startMaximized": true,
    "defaultFolder": "",
    "appScale": 1.0
}
```

### Storage Location
- **Windows**: `%AppData%\LiteClip\user-settings.json`
- **macOS**: `~/Library/Application Support/LiteClip/user-settings.json`
- **Linux**: `~/.config/LiteClip/user-settings.json`

### Temp File Locations
- **Uploads**: `%LocalAppData%\LiteClip\temp\uploads\`
- **Outputs**: `%LocalAppData%\LiteClip\temp\outputs\`
- **FFmpeg**: `%LocalAppData%\LiteClip\ffmpeg\` (fallback)

---

## 9. FFmpeg Integration

### Encoder Configurations

#### H.264 Encoders
| Encoder | Type | Preset (Fast) | Preset (Quality) |
|---------|------|---------------|------------------|
| libx264 | Software | fast | slow |
| h264_nvenc | NVIDIA | p3 | p7 |
| h264_qsv | Intel | veryfast | slower |
| h264_amf | AMD | speed | quality |

#### H.265 Encoders
| Encoder | Type | Preset (Fast) | Preset (Quality) |
|---------|------|---------------|------------------|
| libx265 | Software | fast | slow |
| hevc_nvenc | NVIDIA | p3 | p7 |
| hevc_qsv | Intel | veryfast | slower |
| hevc_amf | AMD | speed | quality |

### FFmpeg Argument Building

```csharp
// Base arguments
-y -hide_banner -loglevel warning -stats

// Input (with seeking for trimmed videos)
-ss <start_time> -t <duration> -i <input_file>

// Video encoding
-c:v <encoder>
-b:v <bitrate>k
-maxrate <maxrate>k
-bufsize <buffer>k
[encoder-specific args from EncodingModeConfig]

// Scaling filter (if needed)
-vf "scale=iw*<percent>/100:ih*<percent>/100:flags=lanczos"

// Audio
-c:a aac -b:a 128k  // or -an for muted

// Container
-movflags +faststart
-f mp4

// Output
<output_file>
```

### Progress Parsing
FFmpeg stderr is parsed for progress updates:
```
frame=  123 fps= 45 q=28.0 size=    1234kB time=00:00:05.12 bitrate= 1975.2kbits/s speed=1.5x
```

Extracted: `time` → calculate percent from duration → estimate ETA from speed

---

## 10. Avalonia Migration Strategy

### Architecture Changes

```
CURRENT (Photino + ASP.NET)          AVALONIA TARGET
─────────────────────────────        ─────────────────────────────
┌─────────────────────────┐          ┌─────────────────────────┐
│   Photino Window        │          │   Avalonia Window       │
│   (WebView2)            │          │   (Native XAML)         │
│   ┌─────────────────┐   │          │   ┌─────────────────┐   │
│   │ Svelte Frontend │   │          │   │ XAML Views      │   │
│   │ (HTML/CSS/JS)   │   │          │   │ (Native UI)     │   │
│   └─────────────────┘   │          │   └─────────────────┘   │
└───────────┬─────────────┘          └───────────┬─────────────┘
            │ HTTP                               │ Direct Call
            ▼                                    ▼
┌─────────────────────────┐          ┌─────────────────────────┐
│   ASP.NET Core Server   │          │   ViewModels (MVVM)     │
│   (Kestrel + Endpoints) │          │   - MainViewModel       │
│                         │    ──►   │   - CompressionVM       │
│   ┌─────────────────┐   │          │   - SettingsVM          │
│   │ Services (DI)   │   │          │   - VideoEditorVM       │
│   └─────────────────┘   │          └───────────┬─────────────┘
└─────────────────────────┘                      │ Direct Call
                                                 ▼
                                     ┌─────────────────────────┐
                                     │   Services (DI)         │
                                     │   (UNCHANGED)           │
                                     └─────────────────────────┘
```

### What Changes

| Component | Current | Avalonia |
|-----------|---------|----------|
| Window Host | Photino.NET | Avalonia Window |
| UI Framework | Svelte 5 | Avalonia XAML |
| UI Pattern | Component-based | MVVM |
| State Management | Svelte stores | ReactiveUI / CommunityToolkit.Mvvm |
| Styling | CSS | Avalonia Styles |
| Communication | HTTP API | Direct method calls |
| File Upload | FormData POST | Direct file path |
| Progress Updates | Polling | IProgress<T> / Events |

### What Stays the Same

| Component | Notes |
|-----------|-------|
| All Services | `VideoCompressionService`, `FfmpegBootstrapper`, etc. |
| Models | `CompressionRequest`, `CompressionJob`, etc. |
| FFmpeg Integration | Process execution, argument building |
| Compression Strategies | `ICompressionStrategy`, `EncodingModeConfigs` |
| Settings Storage | `UserSettingsStore` |
| Bitrate Calculations | `DefaultCompressionPlanner` |

### Key Migration Tasks

1. **Remove HTTP Layer**
   - Delete `Endpoints/` folder
   - Remove Kestrel configuration
   - Remove `IFormFile` handling (use file paths directly)

2. **Create ViewModels**
   - `MainViewModel` - orchestrates app state
   - `CompressionViewModel` - handles compression workflow
   - `VideoEditorViewModel` - handles video editing
   - `SettingsViewModel` - handles settings

3. **Adapt Services**
   - Change `CompressVideoAsync(IFormFile, ...)` to `CompressVideoAsync(string filePath, ...)`
   - Add `IProgress<CompressionProgress>` parameter for progress reporting
   - Remove HTTP-specific code

4. **Create Views**
   - `MainWindow.axaml` - main application window
   - `UploadView.axaml` - file selection
   - `VideoEditorView.axaml` - video trimming
   - `CompressionView.axaml` - progress display
   - `SettingsView.axaml` - settings dialog

5. **Handle Threading**
   - Use `Dispatcher.UIThread` for UI updates
   - Keep FFmpeg processing on background threads
   - Use async/await properly

---

## 11. Component Mapping

### Svelte → Avalonia View Mapping

| Svelte Component | Avalonia View | Notes |
|------------------|---------------|-------|
| `App.svelte` | `MainWindow.axaml` | Main container |
| `UploadArea.svelte` | `UploadView.axaml` | Use `DragDrop` |
| `Sidebar.svelte` | `SettingsPanel.axaml` | Use `StackPanel` |
| `ProgressCard.svelte` | `ProgressView.axaml` | Use `ProgressBar` |
| `StatusCard.svelte` | `StatusView.axaml` | Use `TextBlock` |
| `OutputPanel.svelte` | `OutputView.axaml` | Download button |
| `VideoEditor.svelte` | `VideoEditorView.axaml` | Complex - see below |
| `SettingsModal.svelte` | `SettingsWindow.axaml` | Modal dialog |
| `FfmpegOverlay.svelte` | `FfmpegOverlay.axaml` | Overlay panel |
| `Header.svelte` | Part of `MainWindow` | Title bar area |

### Video Editor Components

| Svelte Component | Avalonia Equivalent |
|------------------|---------------------|
| `VideoPreview.svelte` | `LibVLCSharp.Avalonia` or custom `Image` with frame extraction |
| `Timeline.svelte` | Custom `Canvas` or `ItemsControl` |
| `SegmentsList.svelte` | `ListBox` with custom template |

### State → ViewModel Mapping

| Svelte State | ViewModel Property | Type |
|--------------|-------------------|------|
| `selectedFile` | `SelectedFilePath` | `string?` |
| `sourceDuration` | `SourceDuration` | `TimeSpan?` |
| `sourceVideoWidth/Height` | `SourceResolution` | `Size?` |
| `videoSegments` | `Segments` | `ObservableCollection<VideoSegment>` |
| `jobId` | `CurrentJobId` | `string?` |
| `isCompressing` | `IsCompressing` | `bool` |
| `progressPercent` | `Progress` | `double` |
| `codecSelectValue` | `SelectedCodec` | `CodecKey` |
| `outputSizeSliderValue` | `TargetSizeMb` | `double` |
| `muteAudio` | `MuteAudio` | `bool` |
| `resolutionPreset` | `ResolutionPreset` | `ResolutionPreset` |

---

## 12. Migration Checklist

### Phase 1: Project Setup
- [ ] Create new Avalonia project (`Avalonia.Desktop` template)
- [ ] Add NuGet packages:
  - [ ] `Avalonia.Desktop`
  - [ ] `Avalonia.Themes.Fluent`
  - [ ] `CommunityToolkit.Mvvm` or `ReactiveUI`
  - [ ] `Xabe.FFmpeg.Downloader`
  - [ ] `LibVLCSharp.Avalonia` (optional, for video preview)
- [ ] Copy over existing services, models, strategies
- [ ] Set up DI container (use `Microsoft.Extensions.DependencyInjection`)

### Phase 2: Core Services Adaptation
- [ ] Modify `VideoCompressionService`:
  - [ ] Change `IFormFile` to `string filePath`
  - [ ] Add `IProgress<CompressionProgress>` parameter
  - [ ] Remove HTTP-specific code
- [ ] Keep all other services unchanged
- [ ] Test services in isolation

### Phase 3: ViewModels
- [ ] Create `MainViewModel`
- [ ] Create `CompressionViewModel`
- [ ] Create `VideoEditorViewModel`
- [ ] Create `SettingsViewModel`
- [ ] Implement `INotifyPropertyChanged` / `ObservableObject`
- [ ] Wire up commands (`ICommand` / `RelayCommand`)

### Phase 4: Views
- [ ] Create `MainWindow.axaml`
- [ ] Create `UploadView.axaml` with drag-drop
- [ ] Create `SettingsPanel.axaml`
- [ ] Create `ProgressView.axaml`
- [ ] Create `OutputView.axaml`
- [ ] Create `SettingsWindow.axaml`
- [ ] Create `FfmpegOverlay.axaml`

### Phase 5: Video Editor
- [ ] Evaluate video playback options:
  - [ ] LibVLCSharp.Avalonia
  - [ ] FFmpeg frame extraction to `Image`
  - [ ] Native platform APIs
- [ ] Create `VideoEditorView.axaml`
- [ ] Implement timeline control
- [ ] Implement segment selection

### Phase 6: Styling
- [ ] Apply Fluent theme
- [ ] Create custom styles matching current design
- [ ] Implement dark/light mode support
- [ ] Add animations for state transitions

### Phase 7: Platform Integration
- [ ] File dialogs (open/save)
- [ ] Drag-and-drop file handling
- [ ] Window state persistence
- [ ] System tray (optional)

### Phase 8: Testing & Polish
- [ ] Test all compression scenarios
- [ ] Test hardware encoder detection
- [ ] Test video editor functionality
- [ ] Performance optimization
- [ ] Memory leak checking
- [ ] Cross-platform testing (Windows, macOS, Linux)

### Phase 9: Build & Distribution
- [ ] Configure single-file publish
- [ ] Create installer scripts
- [ ] Update CI/CD pipeline
- [ ] Update documentation

---

## Appendix: File Structure Comparison

### Current Structure
```
liteclip/
├── Program.cs                    # Entry point, Photino + Kestrel setup
├── Endpoints/                    # HTTP API endpoints (DELETE in Avalonia)
│   ├── CompressionEndpoints.cs
│   ├── SettingsEndpoints.cs
│   └── SystemEndpoints.cs
├── Services/                     # Business logic (KEEP)
├── Models/                       # Data models (KEEP)
├── CompressionStrategies/        # Encoding strategies (KEEP)
├── Serialization/                # JSON serialization (KEEP)
├── frontend/                     # Svelte app (REPLACE with Avalonia views)
└── wwwroot/                      # Built frontend (DELETE)
```

### Proposed Avalonia Structure
```
liteclip/
├── App.axaml                     # Application definition
├── App.axaml.cs                  # Application code-behind
├── Program.cs                    # Entry point (simplified)
├── Views/                        # Avalonia XAML views
│   ├── MainWindow.axaml
│   ├── UploadView.axaml
│   ├── CompressionView.axaml
│   ├── VideoEditorView.axaml
│   ├── SettingsWindow.axaml
│   └── Controls/                 # Custom controls
│       ├── Timeline.axaml
│       └── SegmentMarker.axaml
├── ViewModels/                   # MVVM ViewModels
│   ├── MainViewModel.cs
│   ├── CompressionViewModel.cs
│   ├── VideoEditorViewModel.cs
│   └── SettingsViewModel.cs
├── Services/                     # Business logic (KEEP, minor adaptations)
├── Models/                       # Data models (KEEP)
├── CompressionStrategies/        # Encoding strategies (KEEP)
├── Serialization/                # JSON serialization (KEEP)
├── Styles/                       # Avalonia styles
│   └── App.axaml
└── Assets/                       # Icons, images
    └── logo.ico
```

---

## Summary

This specification documents the complete architecture of LiteClip for migration to Avalonia. The key insight is that **most of the business logic (services, models, strategies) can be reused unchanged**. The migration primarily involves:

1. **Replacing the UI layer** (Svelte → Avalonia XAML)
2. **Removing the HTTP layer** (ASP.NET endpoints → direct ViewModel calls)
3. **Adapting the entry point** (Photino → Avalonia Application)

The compression pipeline, FFmpeg integration, encoder selection, and all core algorithms remain identical. This makes the migration a UI-focused effort rather than a complete rewrite.
