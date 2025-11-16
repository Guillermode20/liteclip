# LiteClip Build Instructions

This document describes how to build LiteClip packages for Linux (Fedora) and Windows platforms.

## Version

Current version: **1.0.0**

## Prerequisites

### For Linux Builds

- **Bash shell**
- **.NET 9.0 SDK** or later: https://dotnet.microsoft.com/download/dotnet/9.0
- **Node.js 18+**: https://nodejs.org/
- **rpm-build** (for RPM packages): `sudo dnf install rpm-build rpmdevtools`
- **wget** (for AppImage): `sudo dnf install wget`

### For Windows Builds

- **PowerShell** (pre-installed on Windows)
- **.NET 9.0 SDK** or later: https://dotnet.microsoft.com/download/dotnet/9.0
- **Node.js 18+**: https://nodejs.org/
- **Inno Setup 6** (for installer): https://jrsoftware.org/isdl.php

## Build Scripts Overview

All build outputs are placed in the `dist/` directory.

### Linux Scripts

| Script | Output | Description |
|--------|--------|-------------|
| `build-linux.sh` | `publish-linux/liteclip` | Builds the Linux binary |
| `build-appimage.sh` | `dist/LiteClip-1.0.0-x86_64.AppImage` | Creates portable AppImage |
| `build-rpm.sh` | `dist/liteclip-1.0.0-1.fc42.x86_64.rpm` | Creates RPM installer |

### Windows Scripts

| Script | Output | Description |
|--------|--------|-------------|
| `publish-win.ps1` | `publish-win/liteclip.exe` and `dist/liteclip-1.0.0-portable-win-x64.exe` | Builds portable executable |
| `build-installer.ps1` | `dist/LiteClip-Setup-1.0.0.exe` | Creates Windows installer |
| `build.bat` | Same as `publish-win.ps1` | Wrapper for publish-win.ps1 |

## Building for Linux

### Build Linux Binary

```bash
chmod +x build-linux.sh
./build-linux.sh
```

This creates a self-contained Linux binary in `publish-linux/liteclip`.

### Build AppImage (Portable)

```bash
chmod +x build-appimage.sh
./build-appimage.sh
```

This creates `dist/LiteClip-1.0.0-x86_64.AppImage` - a portable Linux application that runs on any distribution.

**Usage:**
```bash
chmod +x dist/LiteClip-1.0.0-x86_64.AppImage
./dist/LiteClip-1.0.0-x86_64.AppImage
```

### Build RPM Package (Fedora Installer)

```bash
chmod +x build-rpm.sh
./build-rpm.sh
```

This creates `dist/liteclip-1.0.0-1.fc42.x86_64.rpm` - an installable RPM package for Fedora.

**Installation:**
```bash
sudo dnf install dist/liteclip-1.0.0-1.fc42.x86_64.rpm
```

Or:
```bash
sudo rpm -ivh dist/liteclip-1.0.0-1.fc42.x86_64.rpm
```

**Running after installation:**
```bash
liteclip
```

## Building for Windows

### Build Portable Executable

```powershell
.\build.bat
```

Or directly:
```powershell
.\publish-win.ps1
```

- By default, `dotnet publish -c Release` will produce a single-file executable in the `publish-win/` folder.
This creates:
- `publish-win/liteclip.exe` - Working directory build
- `dist/liteclip-1.0.0-portable-win-x64.exe` - Versioned portable copy

The portable executable can be copied anywhere and run directly (no installation required).

### Build Windows Installer

```powershell
.\build-installer.ps1
```

This creates `dist/LiteClip-Setup-1.0.0.exe` - an installer that:
- Installs LiteClip to Program Files
- Creates Start Menu shortcuts
- Includes an uninstaller
- Shows FFmpeg installation reminder

**Optional:** Specify custom Inno Setup path:
```powershell
.\build-installer.ps1 -InnoSetupPath "C:\Path\To\ISCC.exe"
```

## Dependencies

### Runtime Dependencies

All packages are **self-contained** and include the .NET runtime. Users do NOT need to install .NET separately.

#### Linux
- **webkit2gtk4.0**: Required for Photino.NET UI
  ```bash
  sudo dnf install webkit2gtk4.0
  ```
- **FFmpeg**: Required for video compression (not bundled)
  ```bash
  sudo dnf install ffmpeg
  ```

#### Windows
- **WebView2 Runtime**: Usually pre-installed on Windows 10/11
  - If missing, download from: https://developer.microsoft.com/microsoft-edge/webview2/
- **FFmpeg**: Required for video compression (not bundled)
  - Download from: https://ffmpeg.org/download.html
  - Add to system PATH or place `ffmpeg.exe` next to `liteclip.exe`

## Directory Structure

```
smart-compressor/
├── dist/                                    # All build outputs
│   ├── .gitkeep
│   ├── LiteClip-1.0.0-x86_64.AppImage      # Linux portable (created by build-appimage.sh)
│   ├── liteclip-1.0.0-1.fc42.x86_64.rpm    # Fedora RPM (created by build-rpm.sh)
│   ├── liteclip-1.0.0-portable-win-x64.exe # Windows portable (created by publish-win.ps1)
│   └── LiteClip-Setup-1.0.0.exe            # Windows installer (created by build-installer.ps1)
├── publish-linux/                           # Linux build output
│   └── liteclip
├── publish-win/                             # Windows build output
│   └── liteclip.exe
├── build-linux.sh                           # Linux build script
├── build-appimage.sh                        # AppImage packager
├── build-rpm.sh                             # RPM packager
├── liteclip.spec                            # RPM spec file
├── build-installer.ps1                      # Windows installer builder
├── liteclip-installer.iss                   # Inno Setup script
├── build.bat                                # Windows build launcher
└── publish-win.ps1                          # Windows build script
```

## Build All Packages

### On Linux (Fedora)
```bash
# Make scripts executable
chmod +x build-linux.sh build-appimage.sh build-rpm.sh

# Build all Linux packages
./build-appimage.sh  # Also builds Linux binary
./build-rpm.sh       # Also builds Linux binary
```

### On Windows
```powershell
# Build portable and installer
.\build-installer.ps1  # Also builds portable exe
```

### Cross-Platform Notes
- Linux packages must be built on Linux
- Windows packages must be built on Windows
- The .NET project itself is cross-platform compatible

### Frontend Build Integration
- The Svelte/Vite frontend is automatically built as part of `dotnet build` and `dotnet publish`.
- Output goes to `wwwroot/` (configured in `frontend/vite.config.ts`).
- Manual UI build (optional):
  ```powershell
  cd frontend
  npm install   # first time only
  npm run build
  ```

## Troubleshooting

### Linux

**Issue:** `dotnet: command not found`
- **Solution:** Install .NET 9.0 SDK: https://dotnet.microsoft.com/download/dotnet/9.0

**Issue:** `rpmbuild: command not found`
- **Solution:** `sudo dnf install rpm-build rpmdevtools`

**Issue:** AppImage build fails
- **Solution:** Ensure wget is installed: `sudo dnf install wget`

### Windows

**Issue:** `Inno Setup Compiler not found`
- **Solution:** Install Inno Setup 6 from https://jrsoftware.org/isdl.php

**Issue:** `dotnet: command not found`
- **Solution:** Install .NET 9.0 SDK: https://dotnet.microsoft.com/download/dotnet/9.0

**Issue:** `node: command not found`
- **Solution:** Install Node.js from https://nodejs.org/

## FFmpeg Notes

FFmpeg is **NOT** bundled with any package to keep them lightweight. Users must install FFmpeg separately:

### Linux
```bash
sudo dnf install ffmpeg
```

### Windows
1. Download FFmpeg from https://ffmpeg.org/download.html
2. Extract and add to system PATH
3. OR place `ffmpeg.exe` in the same directory as `liteclip.exe`

The application will automatically detect FFmpeg from:
1. Configuration file setting (`appsettings.json`)
2. System PATH
3. Same directory as executable

## Version Updates

To update the version number for future releases:

1. Update version in each build script:
   - `build-linux.sh`: Update `VERSION` variable
   - `build-appimage.sh`: Update `VERSION` variable
   - `build-rpm.sh`: Update `VERSION` variable
   - `liteclip.spec`: Update `Version` field
   - `publish-win.ps1`: Update `$Version` variable
   - `build-installer.ps1`: Update `$VERSION` variable
   - `liteclip-installer.iss`: Update `#define MyAppVersion`

2. Update `liteclip.spec` changelog section

## License

Provided as-is for personal and educational use.

