# Shell and Platform Rules

## PowerShell Requirement

- **ALWAYS use PowerShell** for all shell commands, never bash
- Use PowerShell cmdlets and syntax
- Windows is the only supported platform

## Platform-Specific Paths

- Use backslashes for paths (Windows standard)
- Environment variables use `$env:VARNAME` syntax in PowerShell
- Config path: `%APPDATA%\liteclip-replay\liteclip-replay.toml`

## FFmpeg Locations (checked in order)

1. `LITECLIP_FFMPEG_PATH` environment variable
2. `./ffmpeg/bin/ffmpeg.exe` (relative to exe)
3. `<exe_dir>/ffmpeg/bin/ffmpeg.exe` (beside executable)
4. System PATH
