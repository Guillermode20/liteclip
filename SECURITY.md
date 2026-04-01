# Security

This is a **local-only** Windows application with no network functionality, user accounts, or cloud services.

## What This Means

- **No network ports opened**
- **No data transmitted to external servers**
- **No telemetry or analytics**
- All recordings stay on your local machine in `%APPDATA%\liteclip`

## Security Considerations

### Permissions Required

The app requires standard permissions for:
- **Screen capture** - Uses Windows DXGI Desktop Duplication API
- **Audio recording** - Uses WASAPI for system audio and microphone
- **File system** - Writes video files to your configured clips folder
- **Registry (optional)** - Auto-start functionality writes to `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`

### Download Safety

Only download from the [official releases page](https://github.com/Guillermode20/liteclip-recorder/releases).

## Reporting Issues

If you discover a security issue, please open a private discussion on GitHub or email the maintainers directly rather than posting publicly.
