# LiteClip

A fast, lightweight desktop application for compressing videos. Built with ASP.NET Core, Svelte, and WebView2—no browser needed.

## Quick Start

1. Download `liteclip.exe` from releases
2. Ensure FFmpeg is installed and available in PATH (or place `ffmpeg.exe` in the `ffmpeg/` directory)
3. Run the executable—a native window opens automatically
4. Upload a video (drag & drop or file picker)
5. Adjust the target size slider and pick a codec
6. Compress and download

## Features

- **Codec Selection**: H.264, H.265, VP9, AV1
- **Target Size Slider**: Drag to set compression target (1-100% of original)
- **Automatic Optimization**: Resolution scales automatically to hit target size
- **Video Preview**: Play compressed result before downloading
- **Drag & Drop Upload**: Easy file selection
- **Real-Time Progress**: Live status with ETA during compression
- **Native Desktop Window**: WebView2-based UI, no browser required
- **Single Executable**: Self-contained app

## System Requirements

- **Windows 10/11** (64-bit)
- **WebView2 Runtime** (usually pre-installed; if missing, download [here](https://developer.microsoft.com/en-us/microsoft-edge/webview2/#download-section))
- **FFmpeg**: Install via your package manager or download from [ffmpeg.org](https://ffmpeg.org/download.html)

## Developer Setup

### Prerequisites

- [.NET 9.0 SDK](https://dotnet.microsoft.com/download/dotnet/9.0) or later
- [Node.js](https://nodejs.org/) 18+
- [FFmpeg](https://ffmpeg.org/) executable at `ffmpeg/ffmpeg.exe`

### Build

**Windows (PowerShell or Command Prompt):**
```powershell
.\build.bat
```

Or manually (frontend builds automatically during .NET build/publish):
```powershell
# Publish a Release single-file to the project's publish-win directory
dotnet publish -c Release
```

Notes:
- `dotnet build -c Release` will compile the code, but to generate a single-file, use `dotnet publish -c Release`.

Optional: rebuild UI only
```powershell
cd frontend
npm install   # first time only
npm run build
```

Output: `publish-win/liteclip.exe`

### Development

Run the app with the native window:
```bash
dotnet run
```

For live frontend editing (requires commenting out WebView code in `Program.cs`):
```bash
# Terminal 1
dotnet run

# Terminal 2
cd frontend && npm run dev
```

Backend runs on a dynamic port (shown in console); frontend dev server typically at `http://localhost:5173`.

### Project Layout

```
liteclip/
├── Program.cs                 # ASP.NET Core app & WebView2 setup
├── Services/
│   ├── VideoCompressionService.cs    # Core compression logic
│   ├── FfmpegPathResolver.cs         # FFmpeg binary locator
│   └── JobCleanupService.cs          # Background cleanup
├── Models/                    # CompressionRequest, CompressionResult
├── frontend/                  # Svelte UI (Vite-built)
│   └── src/App.svelte        # Main component
├── ffmpeg/                    # FFmpeg binaries (embedded in release)
├── wwwroot/                   # Built UI assets (embedded)
├── appsettings.json           # Configuration
└── liteclip.csproj
```

### Configuration

Edit `appsettings.json`:

```json
{
  "Compression": {
    "MaxConcurrentJobs": 2,
    "MaxQueueSize": 10,
    "JobRetentionMinutes": 30
  },
  "FileUpload": {
    "MaxFileSizeBytes": 2147483648
  },
  "TempPaths": {
    "Uploads": "temp/uploads",
    "Outputs": "temp/outputs"
  }
}
```

### API Endpoints

- `POST /api/compress` – Upload and start compression
- `GET /api/status/{jobId}` – Check job status
- `GET /api/download/{jobId}` – Download compressed video
- `POST /api/cancel/{jobId}` – Cancel a job

## Tech Stack

- **Backend**: ASP.NET Core 9.0
- **Frontend**: Svelte 5 + TypeScript
- **Build**: Vite
- **Video Processing**: FFmpeg
- **Desktop UI**: WebView2 (native Windows control)

## License

Provided as-is for personal and educational use.

