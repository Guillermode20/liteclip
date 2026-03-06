# LiteClip Replay — WiX v4 Installer

This folder contains a WiX Toolset v4 installer project for LiteClip Replay.

## What is included
- `LiteClipReplay.wixproj` — MSBuild-based WiX v4 project (x64, Debug/Release support)
- `Product.wxs`, `Directories.wxs`, `Components.wxs`, `Features.wxs`, `Shortcuts.wxs`, `Registry.wxs`, `UI.wxs` — WiX fragments
- `Harvest.targets` — runs `heat.exe` to harvest optional `ffmpeg/bin/*` files into WiX fragments
- `Bundle.wxs` — bootstrapper (Burn) stub chaining prerequisites + MSI
- `License.rtf` — license placeholder (MIT)
- `en-US.wxl`, `Variables.wxs` — localization / variables
- `build.ps1`, `build.cmd` — build scripts that run `cargo` then MSBuild

## Build (local)
1. Ensure WiX Toolset v4 and dotnet SDK are available (WiX MSBuild package is referenced by the project).
2. Build the Rust binary:
   cargo build --release --features ffmpeg
3. Build the installer from `installer/`:
   dotnet msbuild installer\LiteClipReplay.wixproj /p:Configuration=Release /p:ProductVersion=1.0.0.0

Or use the convenience script in `installer/`:
   powershell -ExecutionPolicy Bypass -File installer\build.ps1

## CI / Versioning
- Override `ProductVersion` via MSBuild `/p:ProductVersion=1.2.3.0`.
- `Harvest.targets` uses `heat.exe` to gather files from `target/release` and `ffmpeg/bin`.

## Notes & validation checklist
- All component GUIDs are explicit and unique.
- Installer scope is per-machine only.
- FFmpeg payload: DLLs are harvested from `..\ffmpeg\bin\` for native `ffmpeg-next` linking. Required DLLs include avcodec, avformat, avutil, swscale, swresample, etc.
- File association `.lcr` is registered per-machine.
- Desktop shortcut is optional (feature-controlled).

If you want, I can:
- Add icon resources and include product icon in Start Menu.
- Add code-signing steps to the pipeline (SignTool integration).
- Expand bootstrapper to download/verify prerequisites rather than bundling them.