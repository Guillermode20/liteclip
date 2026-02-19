@echo off
setlocal enabledelayedexpansion

:: Build Rust release with ffmpeg feature
cargo build --release --features ffmpeg
if errorlevel 1 goto :err

:: Restore NuGet packages for WiX project, then build
dotnet restore LiteClipReplay.wixproj
if errorlevel 1 goto :err

:: Build WiX MSI (requires dotnet + WixToolset MSBuild package restored by NuGet)
dotnet msbuild LiteClipReplay.wixproj /t:Build /p:Configuration=Release /p:Platform=x64
if errorlevel 1 goto :err

echo Build complete. MSI located in installer\output
exit /b 0

:err
echo Build failed.
exit /b 1
