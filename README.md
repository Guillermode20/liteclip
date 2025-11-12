# Smart Video Compressor

A fast, lightweight cross-platform desktop application for compressing videos. Built with ASP.NET Core backend, Svelte frontend, and Tauri.

## Quick Start

1. Download the installer for your platform from releases
2. Ensure FFmpeg is installed and available in PATH (or place `ffmpeg` in the `ffmpeg/` directory)
3. Run the application—a native window opens automatically
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
- **Native Desktop Window**: Tauri-based UI for cross-platform support
- **Cross-Platform**: Windows, macOS, and Linux support
- **Lightweight**: Fast startup and minimal resource usage

## System Requirements

- **Windows 10/11** (64-bit), **macOS 10.15+**, or **Linux** (modern distros)
- **FFmpeg**: Install via your package manager or download from [ffmpeg.org](https://ffmpeg.org/download.html)

## Developer Setup

### Prerequisites

- [.NET 9.0 SDK](https://dotnet.microsoft.com/download/dotnet/9.0) or later
- [Node.js](https://nodejs.org/) 18+
- [Rust](https://www.rust-lang.org/tools/install) (for Tauri)
- [FFmpeg](https://ffmpeg.org/) executable in PATH or at `ffmpeg/ffmpeg` (or `ffmpeg/ffmpeg.exe` on Windows)

### Build

**All Platforms:**

For production build:
```bash
# Windows
.\build-tauri.ps1 -Release

# macOS/Linux
chmod +x build-tauri.sh
./build-tauri.sh --release
```

For debug build:
```bash
# Windows
.\build-tauri.ps1

# macOS/Linux
./build-tauri.sh
```

Output locations:
- **Release**: `tauri/src-tauri/target/release/bundle/` (platform-specific installers)
- **Debug**: `tauri/src-tauri/target/debug/`

### Development

Run the app in development mode with hot-reload:

```bash
cd tauri
npm install
npm run tauri dev
```

This will:
1. Start the Vite dev server for the frontend (port 5173)
2. Build and run the Tauri app
3. Automatically spawn the .NET backend (port 5333)
4. Enable hot-reload for frontend changes

The frontend proxies API requests to `http://localhost:5333` where the backend runs.

### Project Layout

```
smart-compressor/
├── Program.cs                 # ASP.NET Core API backend
├── Services/
│   ├── VideoCompressionService.cs    # Core compression logic
│   ├── FfmpegPathResolver.cs         # FFmpeg binary locator
│   └── JobCleanupService.cs          # Background cleanup
├── Models/                    # CompressionRequest, CompressionResult
├── CompressionStrategies/     # Codec-specific strategies
├── tauri/                     # Tauri frontend application
│   ├── src/                   # Svelte UI source
│   │   ├── App.svelte        # Main component
│   │   ├── main.ts           # Entry point
│   │   └── app.css           # Styles
│   ├── src-tauri/             # Rust/Tauri backend
│   │   ├── src/lib.rs        # Backend lifecycle management
│   │   ├── Cargo.toml        # Rust dependencies
│   │   ├── tauri.conf.json   # Tauri configuration
│   │   └── binaries/         # Sidecar binaries (built .NET backend)
│   ├── package.json           # Frontend dependencies
│   └── vite.config.js         # Vite configuration
├── ffmpeg/                    # FFmpeg binaries (for development)
├── appsettings.json           # Backend configuration
├── smart-compressor.csproj    # Backend build config
├── build-tauri.ps1            # Windows build script
└── build-tauri.sh             # macOS/Linux build script
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

- `GET /api/health` – Health check endpoint
- `POST /api/compress` – Upload and start compression
- `GET /api/status/{jobId}` – Check job status
- `GET /api/download/{jobId}` – Download compressed video
- `POST /api/cancel/{jobId}` – Cancel a job

## Tech Stack

- **Backend**: ASP.NET Core 9.0 (API server)
- **Frontend**: Svelte 5 + TypeScript
- **Desktop Framework**: Tauri 2.x (Rust)
- **Build**: Vite
- **Video Processing**: FFmpeg

## Architecture

The application uses a **hybrid architecture**:

1. **Tauri Frontend**: Manages the desktop window and serves the Svelte UI
2. **.NET Backend**: Runs as a Tauri sidecar process, providing video compression APIs
3. **Communication**: Frontend makes HTTP requests to backend at `localhost:5333`

This architecture provides:
- Cross-platform support (Windows, macOS, Linux)
- Lightweight native windows via Tauri
- Powerful video processing via .NET and FFmpeg
- Modern reactive UI with Svelte

## License

Provided as-is for personal and educational use.

