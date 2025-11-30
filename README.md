<div align="center">
  <img src="frontend/public/logo.svg" alt="liteclip logo" width="120" height="120">
  <h1>liteclip</h1>
  <p><strong>super simple desktop video editor & compressor.</strong></p>
</div>

## make heavy clips feel lite

liteclip is a minimal desktop app that trims, edits, and compresses videos so they fit under stubborn file size limits (discord, whatsapp, instagram reels, email, etc.). there are no confusing menus—just drag, edit, compress, and share. everything runs locally on your machine using ffmpeg under the hood, so your footage never leaves your device.

### key things you get

- **simple editing** – trim, split, and merge segments without a complicated timeline.
- **target file size or quality presets** – dial in the exact megabytes you need or pick a preset and let liteclip do the math.
- **fast, efficient compression** – automatically uses your gpu when possible for quicker exports.
- **local & private** – no uploads, no servers. photino + asp.net core keep the entire workflow offline.
- **tiny download** – ~49 mb installer with no bundled bloat. currently shipping for windows (more platforms planned).

## downloads

- ▶️ [Download for Windows (Installer)](https://github.com/Guillermode20/liteclip/releases/download/v0.1.0/LiteClip-Setup.exe)
- 📦 [See all releases](../../releases) for portable builds, checksums, and release notes.

## quick start

1. download and install liteclip.
2. launch the app (a native window opens automatically).
3. drag & drop a video or use the file picker to import it.
4. trim, split, or merge clips to keep only what matters.
5. set a target file size or choose a quality preset.
6. hit **compress** and save the new lite file.

## use cases

- keep discord uploads under the 8 mb limit without nitro.
- prep reels for tiktok/instagram with predictable sizes.
- shrink videos for email attachments or messaging apps (whatsapp, telegram, imessage, etc.).
- reduce large recordings before syncing to cloud storage.

## key technologies

- **photino.net** hosts the native desktop window so the ui feels like a real app instead of a browser.
- **asp.net core minimal apis** run locally inside the same process to handle uploads, compression requests, and file streaming.
- **ffmpeg** (bootstrapped via `Xabe.FFmpeg.Downloader`) does the heavy lifting for trimming, merging, and gpu-accelerated encoding.
- **svelte 5 + typescript + vite** power the modern frontend that sits inside the photino window.

## for developers

want to build liteclip from source? check out [build.md](build.md) for detailed instructions, including how the svelte frontend is bundled into `wwwroot` during `dotnet build`.

> debug builds launched via `dotnet run` show a console window for logging; release builds run as gui-only for a cleaner desktop feel.

## donations

liteclip stands on two excellent open-source projects: [photino.net](https://www.photino.net/) and [ffmpeg](https://ffmpeg.org/). donations are split 50/50 to support both projects.

## license

this project is open source and available under the **mit license**. include the original copyright notice and license in any copies or substantial portions of the software.

copyright (c) 2025 liteclip contributors
