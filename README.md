# Smart Compressor

An all-in-one desktop video editor and compressor designed for platforms with limited upload sizes. Built with ASP.NET Core, Svelte, and Photino—native window, cross-platform, no browser needed.

Perfect for cutting, editing, and compressing videos to fit strict file size limits on social media platforms, messaging apps, and file sharing services.

## Quick Start

1. Download the executable from releases
2. Ensure FFmpeg is installed and available in PATH
3. Run the executable—a native window opens automatically
4. Upload a video (drag & drop or file picker)
5. Edit: trim, split into segments, and merge parts as needed
6. Set target upload size or choose codec manually
7. Compress and download

## Features

- **Video Editing**: Trim videos, split into segments, and merge multiple parts back together
- **Smart Codec Selection**: Automatic quality-optimized or manual choice (H.264, H.265, VP9, AV1)
- **Hardware Encoder Detection**: Auto-detects and uses NVENC, QSV, AMF when available
- **Target Size Compression**: Set exact output size to meet platform upload limits
- **Automatic Optimization**: Resolution and bitrate scales automatically to hit target size
- **Resolution Presets**: Force 1080p/720p/480p/360p output when you need strict dimensions
- **Two-Pass Encoding**: Accurate bitrate targeting for precise file sizes
- **Audio Control**: One-click mute option to drop audio and save precious megabytes
- **Video Preview**: Play compressed result before downloading
- **Drag & Drop Upload**: Easy file selection
- **Real-Time Progress**: Live status with queue position and ETA during compression
- **Job Queue**: Configurable concurrent compression limit with queue management
- **Retry Safety Net**: Failed jobs can be re-queued instantly without re-uploading files
- **Update Notifications**: Built-in release checker so you always know when a new build ships
- **Cross-Platform**: Runs on Windows, Linux, and macOS
- **Native Desktop Window**: Photino-based UI, no browser required
- **Single Executable**: Self-contained app (Release builds embed UI assets)

## System Requirements

- **Windows 10/11, Linux (GTK3+), or macOS 10.14+** (64-bit)
- **FFmpeg**: Install via your package manager or download from [ffmpeg.org](https://ffmpeg.org/download.html)

## Use Cases

- Compress videos to fit Discord's 8MB limit
- Create TikTok/Instagram Reels under file size restrictions
- Prepare videos for email with attachment limits
- Cut and compress large recordings for cloud storage
- Reduce video file sizes for messaging apps (WhatsApp, Telegram, etc.)

## Developer Setup

### Prerequisites

- [.NET 10.0 SDK](https://dotnet.microsoft.com/download/dotnet/10.0) or later
- [Node.js](https://nodejs.org/) 18+
- [FFmpeg](https://ffmpeg.org/) (ensure `ffmpeg` is in PATH)

### Build

**Cross-Platform (Linux, macOS, Windows):**
```bash
# Release build (self-contained single file)
dotnet publish -c Release -r linux-x64  # or win-x64, osx-x64
```

The frontend builds automatically during .NET build/publish via the `BuildFrontend` MSBuild target.

Output locations:
- Windows: `publish/smart-compressor.exe`
- Linux: `publish/smart-compressor`
- macOS: `publish/smart-compressor`

Optional: rebuild UI only
```bash
cd frontend
npm install   # first time only
npm run build
```

### Development

Run the app with the native window:
```bash
dotnet run
```

The backend runs on a dynamic port (shown in console).

### Project Layout

```
smart-compressor/
├── Program.cs                           # ASP.NET Core minimal API + Photino window
├── Services/
│   ├── VideoCompressionService.cs       # Core compression logic & job queue
│   ├── FfmpegPathResolver.cs            # FFmpeg binary discovery
│   ├── FfmpegCapabilityProbe.cs         # Detects available ffmpeg encoders and capability limits
│   └── JobCleanupService.cs             # Background cleanup of expired jobs
├── CompressionStrategies/               # Strategy pattern for codec-specific logic
│   ├── H264Strategy.cs
│   ├── H265Strategy.cs
│   ├── Vp9Strategy.cs
│   ├── Av1Strategy.cs
│   └── CompressionStrategyFactory.cs
├── Models/                              # CompressionRequest, CompressionResult, CompressionJob
├── frontend/                            # Svelte 5 UI (Vite-built)
│   ├── src/
│   │   ├── App.svelte                   # Main app with editor, upload, progress
│   │   ├── VideoEditor.svelte           # Video segment trimming/merging
│   │   └── components/                  # UI components
│   └── vite.config.ts
├── wwwroot/                             # Built UI assets (embedded in Release)
├── appsettings.json                     # Configuration (concurrency, retention, temp paths)
├── liteclip.csproj                      # Project file with auto-build targets
└── liteclip.sln
```

### Configuration

Edit `appsettings.json` to customize behavior:

```json
{
  "Compression": {
    "MaxConcurrentJobs": 1,
    "MaxQueueSize": 50,
    "JobRetentionMinutes": 30
  },
  "FileUpload": {
    "MaxFileSizeBytes": 2147483648
  },
  "TempPaths": {
    "Uploads": "temp/uploads",
    "Outputs": "temp/outputs"
  },
  "FFmpeg": {
    "Path": null
  }
}
```

**Settings:**
- `MaxConcurrentJobs`: Number of videos to compress simultaneously (default: 1 to avoid system overload)
- `MaxQueueSize`: Maximum jobs in queue before rejecting new uploads
- `JobRetentionMinutes`: How long to keep completed jobs before auto-cleanup
- `MaxFileSizeBytes`: Maximum upload size (default: 2 GB)
- `FFmpeg.Path`: Force a specific FFmpeg executable path (leave null for auto-detection)

### API Endpoints

- `POST /api/compress` – Upload and start compression
- `GET /api/status/{jobId}` – Check job status
- `GET /api/download/{jobId}` – Download compressed video
- `POST /api/cancel/{jobId}` – Cancel a job

## Tech Stack

- **Backend**: ASP.NET Core 10.0, Kestrel server
- **Frontend**: Svelte 5 (runes mode) + TypeScript
- **Build**: Vite (Rolldown)
- **UI Framework**: Photino.NET (cross-platform native window)
- **Video Processing**: FFmpeg with codec-specific strategies
- **Platform Support**: Windows 10+, Linux (GTK3+), macOS 10.14+

## Architecture Highlights

- **Strategy Pattern**: Each codec (H.264, H.265, VP9, AV1) has its own compression strategy with hardware encoder detection
- **Job Queue**: Semaphore-based concurrency control prevents system overload
- **Two-Pass Encoding**: Automatic bitrate calculation with configurable container overhead for precise sizing
- **Cross-Platform**: Single codebase builds to Windows, Linux, and macOS executables
- **Embedded Assets**: Release builds embed UI assets into the executable for true single-file distribution
- **Segment Editing**: Built-in trimming and merging capabilities without requiring external tools

## License

Provided as-is for personal and educational use.
