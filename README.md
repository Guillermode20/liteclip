<div align="center">
  <img src="frontend/public/logo.svg" alt="liteclip logo" width="120" height="120">
  <h1>liteclip</h1>
  <p><strong>the easiest way to compress and edit videos for social media.</strong></p>
</div>

## what is liteclip?

liteclip is a simple, powerful tool designed to help you get your videos onto platforms with strict file size limits. 

ever tried to send a video on discord, whatsapp, or email only to be told "file too large"? liteclip fixes that. it intelligently compresses your video to the exact size you need without ruining the quality. you can also trim out the boring parts before you compress!

## features

- **target file size**: slide the slider to set your desired size (e.g., under 8mb for discord) and it handles the rest.
- **easy editing**: trim, cut, and merge video clips. keep only the highlights.
- **smart compression**: uses advanced technology to keep your video looking crisp, even at small sizes.
- **audio control**: remove sound with one click to save even more space.
- **private & offline**: runs entirely on your computer. no uploading to sketchy websites.
- **windows-native**: currently built and shipped for windows.

## how to use

1. **open liteclip**.
2. **drag & drop** your video file into the window.
3. **edit (optional)**: use the timeline to cut or trim your video.
4. **choose a size**: slide the slider to set the target output in megabytes.
5. **hit compress**: watch it shrink in real-time!

## download

check the [releases](../../releases) page for the latest version for your computer.

## for developers

want to build liteclip from source? check out [build.md](build.md) for detailed instructions.

Note: when running in Debug (e.g. `dotnet run`), the application will show a console window for easier logging and troubleshooting. Release builds are configured as GUI-only (no console) to provide a native desktop experience.

## release workflow

1. update `Properties/AssemblyInfo.cs` with the new semantic version (e.g., `0.1.1`). this value is used by the app, installer, and GitHub release metadata.
2. create a GitHub release (or run the workflow manually) which triggers `.github/workflows/build-installer.yml`.
3. the workflow:
   - builds the app + frontend
   - runs `build-installer/build-installer.ps1` to generate `liteclip-setup.exe` and the portable exe in `dist/`
   - uploads the `dist` folder as a workflow artifact and attaches any `dist/*.exe` files directly to the release
4. publish the release notes (the in-app update checker will surface the new version automatically because it tracks GitHub releases).

## license

this project is open source and available under the **mit license**.

you are free to use, modify, and distribute this software, but you **must include the original copyright notice and license** in any copies or substantial portions of the software.

copyright (c) 2025 liteclip contributors
