# LiteClip Installer Builder

This directory contains the scripts and configuration to build the Windows installer for LiteClip.

## Prerequisites

1.  **Inno Setup 6**: You must have Inno Setup 6 installed.
    *   Download: [https://jrsoftware.org/isdl.php](https://jrsoftware.org/isdl.php)
    *   Default path: `C:\Program Files (x86)\Inno Setup 6\ISCC.exe`

2.  **.NET SDK**: Required to build the application.

3.  **Node.js**: Required to build the frontend.

## How to Build

Run the `build-installer.ps1` script from PowerShell:

```powershell
.\build-installer.ps1
```

This script will:
1.  Run `..\publish-win.ps1` to build and publish the application to `..\publish-win`.
2.  Run `ISCC.exe` with `LiteClip.iss` to compile the installer.
3.  Output the installer to `..\dist`.

## Configuration

*   **LiteClip.iss**: The Inno Setup script. Modify this file to change installer settings, add files, or change the UI.
*   **build-installer.ps1**: The automation script.

## Notes

*   **WebView2**: The installer checks for the WebView2 Runtime and prompts the user to download it if missing.
*   **FFmpeg**: The installer does NOT bundle FFmpeg by default to keep the size small. The application will look for FFmpeg in the system PATH.
