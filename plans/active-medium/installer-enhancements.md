# Plan: Installer Enhancements & Code Signing

## Status
Pending

## Priority
Medium

## Summary
Enhance the WiX MSI installer with code signing, improved bootstrapper for prerequisite downloads, embedded icons, and better user experience during installation.

## Current State
- WiX MSI installer exists in `installer/`
- Installer README mentions potential additions: icon resources, code-signing (SignTool), expanded bootstrapper
- FFmpeg DLLs are copied by build script from `ffmpeg_dev/sdk/bin`
- No code signing -- Windows SmartScreen shows "Unknown Publisher" warning
- No automatic FFmpeg download -- users must provide FFmpeg separately

## Implementation Steps

### 1. Code Signing
- Obtain an EV (Extended Validation) code signing certificate
- Integrate SignTool into the build pipeline
- Sign the main executable and all bundled DLLs
- Sign the MSI installer itself
- Set up timestamping for signature persistence after certificate expiry

### 2. FFmpeg Bootstrapper
- Create a bootstrapper (Burn) that bundles the MSI installer
- Automatically download FFmpeg shared DLLs from a trusted source during install
- Verify downloaded files with checksums
- Fall back to bundled DLLs if download fails
- Support offline installation with pre-downloaded FFmpeg

### 3. Icon and Resource Embedding
- Embed application icon into the executable (already done via `build.rs`)
- Add proper MSI database icons for the installer UI
- Add banner and dialog images for the installer UI

### 4. Installation Experience
- Add installation path selection
- Add option to create desktop shortcut
- Add option to register file associations (`.liteclip` config, clip files)
- Add "Launch LiteClip on finish" checkbox
- Better upgrade experience (preserve config and clips on reinstall)

### 5. CI/CD Integration
- Automate MSI build in GitHub Actions
- Automate code signing with secure certificate storage
- Generate release artifacts: MSI, bootstrapper EXE, checksums

## Files to Modify
- `installer/` -- Enhance WiX configuration, add Burn bootstrapper
- `build.rs` -- Add code signing step
- `.github/workflows/` -- Add installer CI pipeline
- `build.rs` -- Ensure icon embedding is complete

## Estimated Effort
Medium (3-5 days)

## Dependencies
- Code signing certificate (EV recommended for SmartScreen reputation)
- FFmpeg distribution rights and download hosting
- Windows SDK for SignTool

## Risks
- EV certificates are expensive ($200-500/year) and require hardware token
- FFmpeg redistribution requires compliance with LGPL/GPL licensing
- Bootstrapper adds complexity to the installation process
