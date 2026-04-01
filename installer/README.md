# LiteClip — WiX v4 Installer

This folder contains a WiX Toolset v4 installer project for LiteClip.

## What is included
- `LiteClip.wixproj` — MSBuild-based WiX v4 project (x64, Debug/Release support)
- `Product.wxs`, `Directories.wxs`, `Components.wxs`, `Features.wxs`, `Shortcuts.wxs`, `Registry.wxs`, `UI.wxs` — WiX fragments
- `Harvest.targets` — generates WiX fragments for required FFmpeg DLLs from `ffmpeg_dev/sdk/bin`
- `Bundle.wxs` — bootstrapper (Burn) stub chaining prerequisites + MSI
- `License.rtf` — license placeholder (MIT)
- `en-US.wxl`, `Variables.wxs` — localization / variables
- `build.ps1`, `build.cmd` — build scripts that run `cargo`, build the MSI, and create a portable package

## Build (local)
1. Ensure WiX Toolset v4 and dotnet SDK are available (WiX MSBuild package is referenced by the project).
2. Build the Rust binary:
   cargo build --release --features ffmpeg
3. Build the installer from `installer/`:
   dotnet msbuild installer\LiteClip.wixproj /p:Configuration=Release /p:ProductVersion=1.0.0.0

Or use the convenience script in `installer/`:
   powershell -ExecutionPolicy Bypass -File installer\build.ps1

This produces:
- `installer\output\en-US\LiteClip.msi`
- `installer\output\portable\LiteClip-portable\`
- `installer\output\portable\LiteClip-portable.zip`

## CI / Versioning
- Override `ProductVersion` via MSBuild `/p:ProductVersion=1.2.3.0`.
- `Harvest.targets` generates WiX fragments for the required FFmpeg DLLs from `ffmpeg_dev/sdk/bin`.

## Notes & validation checklist
- All component GUIDs are explicit and unique.
- Installer scope is per-machine only.
- FFmpeg payload: Only the required DLLs are included (avcodec-61, avformat-61, avutil-59, swresample-5, swscale-8) from `ffmpeg_dev/sdk/bin/` for native `ffmpeg-next` linking.
- File association `.lcr` is registered per-machine.
- Desktop shortcut is optional (feature-controlled).

If you want, I can:
- Add icon resources and include product icon in Start Menu.
- Add code-signing steps to the pipeline (SignTool integration).
- Expand bootstrapper to download/verify prerequisites rather than bundling them.