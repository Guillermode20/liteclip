# Smart Video Compressor

A powerful, user-friendly desktop application for compressing videos with intelligent quality optimization. Built with ASP.NET Core and Svelte, featuring an embedded FFmpeg engine for professional-grade video compression.

## Features

- **Smart Compression Modes**
  - **Auto Mode**: Intelligently balances quality and file size based on video characteristics
  - **Simple Mode**: Target a specific file size with automatic bitrate calculation
  - **Advanced Mode**: Fine-tune quality using CRF (Constant Rate Factor)

- **Multiple Codec Support**
  - H.264 (Best compatibility)
  - H.265/HEVC (Higher efficiency)
  - VP9 (Web-optimized)
  - AV1 (Next-generation compression)

- **Additional Features**
  - Resolution scaling (10-100%)
  - Two-pass encoding for precise size targeting
  - Real-time progress tracking with ETA
  - Job queuing system
  - Drag-and-drop interface
  - Video preview before and after compression
  - Automatic browser launch

## For End Users

### Quick Start

1. **Download** the latest release (`smart-compressor.exe`)
2. **Run** the executable - your browser will automatically open
3. **Upload** a video file (up to 2GB)
4. **Choose** your compression settings:
   - Select a codec
   - Choose compression mode
   - Adjust target size or quality
5. **Compress** and download your optimized video

### System Requirements

- Windows 10/11 (64-bit)
- No additional software required - FFmpeg is embedded

### Compression Modes Explained

#### Auto Mode (Recommended)
Automatically determines the best compression strategy based on your video's properties. Ideal for most users.

#### Simple Mode
Set a target file size (e.g., 25MB for Discord uploads). The app calculates optimal settings to reach that size while maintaining the best possible quality.

#### Advanced Mode
Full control using CRF values:
- **18-22**: Nearly lossless quality (larger files)
- **23-28**: High quality (balanced, **default: 28**)
- **29-35**: Good quality (smaller files)
- **36-45**: Lower quality (smallest files)

### Tips for Best Results

- Use **H.264** for maximum compatibility
- Use **H.265** for smaller files (50% better compression than H.264)
- Enable **two-pass encoding** when targeting specific file sizes
- Scale resolution to **720p** or **540p** for significant size reduction with minimal quality loss

## For Developers

### Prerequisites

- [.NET 9.0 SDK](https://dotnet.microsoft.com/download/dotnet/9.0) or later
- [Node.js](https://nodejs.org/) 18+ (for building the frontend)
- [FFmpeg](https://ffmpeg.org/) executable (placed in `ffmpeg/ffmpeg.exe` directory)

### Building from Source

#### Windows

1. Clone the repository:
```bash
git clone https://github.com/yourusername/smart-compressor.git
cd smart-compressor
```

2. Run the build script:
```powershell
.\publish-win.ps1
```

The script will:
- Verify .NET SDK and Node.js are installed
- Install frontend dependencies
- Build the Svelte frontend
- Compile and publish the .NET application as a single-file executable
- Embed FFmpeg and the frontend into the executable
- Output to the `publish-win` directory

#### Manual Build Steps

If you prefer to build manually:

1. **Build the frontend:**
```bash
cd frontend
npm install
npm run build
cd ..
```

2. **Publish the .NET application:**
```bash
dotnet publish --configuration Release --runtime win-x64 --self-contained true --output publish-win /p:PublishSingleFile=true
```

3. **Place FFmpeg:**
Ensure `ffmpeg/ffmpeg.exe` exists before building (it will be embedded automatically).

### Development Mode

Run the backend and frontend separately for development:

**Backend:**
```bash
dotnet run
```

**Frontend** (in a separate terminal):
```bash
cd frontend
npm run dev
```

The backend will run on `http://localhost:5000` (or check console output), and the frontend dev server typically runs on `http://localhost:5173`. Update the frontend API base URL if needed.

### Project Structure

```
smart-compressor/
├── Controllers/          # (Future API controllers)
├── Models/              # Data models (CompressionRequest, CompressionResult)
├── Services/            # Core business logic
│   ├── VideoCompressionService.cs    # Main compression engine
│   ├── FfmpegPathResolver.cs         # FFmpeg path management
│   └── JobCleanupService.cs          # Background cleanup service
├── frontend/            # Svelte frontend
│   ├── src/
│   │   ├── App.svelte   # Main application component
│   │   └── main.ts      # Entry point
│   └── vite.config.ts   # Vite configuration
├── ffmpeg/              # FFmpeg binaries (embedded)
├── wwwroot/             # Built frontend assets (embedded)
├── Program.cs           # ASP.NET Core entry point
├── smart-compressor.csproj
└── publish-win.ps1      # Windows build script
```

### Configuration

Settings can be modified in `appsettings.json`:

```json
{
  "Compression": {
    "MaxConcurrentJobs": 2,
    "MaxQueueSize": 10
  },
  "FileUpload": {
    "MaxFileSizeBytes": 2147483648
  },
  "TempPaths": {
    "Uploads": "temp/uploads",
    "Outputs": "temp/outputs"
  },
  "Cleanup": {
    "RetentionMinutes": 60,
    "CleanupIntervalMinutes": 10
  }
}
```

### API Endpoints

- `POST /api/compress` - Upload and compress video
- `GET /api/status/{jobId}` - Check compression status
- `GET /api/download/{jobId}` - Download compressed video
- `POST /api/cancel/{jobId}` - Cancel compression job

### Technology Stack

- **Backend**: ASP.NET Core 9.0 (C#)
- **Frontend**: Svelte 5 + TypeScript
- **Build Tool**: Vite (with Rolldown)
- **Video Processing**: FFmpeg (embedded)
- **Deployment**: Single-file executable with embedded static files

## Contributing

Contributions are welcome! Please feel free to submit issues or pull requests.

## License

This project is provided as-is for personal and educational use.

## Acknowledgments

- Built with [FFmpeg](https://ffmpeg.org/)
- Frontend powered by [Svelte](https://svelte.dev/)
- Backend powered by [ASP.NET Core](https://dotnet.microsoft.com/apps/aspnet)

