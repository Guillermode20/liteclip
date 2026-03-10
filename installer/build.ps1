param(
  [string]$Configuration = "Release",
  [string]$Platform = "x64",
  [string]$ProductVersion = "1.0.0.0",
  [switch]$Sign
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
Push-Location $PSScriptRoot

$releaseDir = Resolve-Path "..\target\release"
$ffmpegBinDir = Join-Path $PSScriptRoot "..\ffmpeg_dev\sdk\bin"
$appExe = Join-Path $releaseDir "liteclip-replay.exe"
$outputDir = Join-Path $PSScriptRoot "output"
$portableRoot = Join-Path $outputDir "portable"
$portableDir = Join-Path $portableRoot "LiteClipReplay-portable"
$portableZip = Join-Path $portableRoot "LiteClipReplay-portable.zip"

Write-Host "1) Building Rust release (ffmpeg feature)"
cargo build --release --features ffmpeg

if (-not (Test-Path $appExe)) {
  throw "Release executable not found at $appExe"
}

# Check for FFmpeg DLLs (required for native ffmpeg-next linking)
if (-not (Test-Path (Join-Path $ffmpegBinDir "avcodec-61.dll"))) {
  Write-Host "Warning: FFmpeg DLLs not found in $ffmpegBinDir. Native FFmpeg may not work correctly."
}

Write-Host "2) Restoring NuGet packages for WiX project"
if (-not (Get-Command heat.exe -ErrorAction SilentlyContinue)) {
  Write-Host "Warning: WiX Toolset (heat.exe) not found in PATH. FFmpeg DLL harvest is skipped."
}
# ensure NuGet packages (WixToolset.MSBuild) are restored before msbuild
dotnet restore .\LiteClipReplay.wixproj

Write-Host "3) Building WiX installer project"
$msbuildStarted = Get-Date
dotnet msbuild .\LiteClipReplay.wixproj /t:Rebuild /p:Configuration=$Configuration /p:Platform=$Platform /p:ProductVersion=$ProductVersion
if ($LASTEXITCODE -ne 0) {
  throw "dotnet msbuild failed with exit code $LASTEXITCODE"
}

# WiX v4 outputs to en-US subdirectory
$msi = Join-Path $PSScriptRoot "output\en-US\LiteClipReplay.msi"
if (Test-Path $msi) {
  $msiInfo = Get-Item $msi
  if ($msiInfo.LastWriteTime -lt $msbuildStarted) {
    throw "MSI exists but was not updated by current build: $msi"
  }
  Write-Host "MSI produced: $msi"
} else {
  Write-Error "MSI not found at $msi"
}

Write-Host "4) Creating portable build"
New-Item -ItemType Directory -Force -Path $portableRoot | Out-Null
if (Test-Path $portableDir) {
  Remove-Item -Recurse -Force $portableDir
}
New-Item -ItemType Directory -Force -Path $portableDir | Out-Null

Copy-Item $appExe -Destination $portableDir

$requiredDlls = @(
  "avcodec-61.dll",
  "avformat-61.dll",
  "avutil-59.dll",
  "swresample-5.dll",
  "swscale-8.dll"
)
foreach ($dll in $requiredDlls) {
  $dllPath = Join-Path $ffmpegBinDir $dll
  if (Test-Path $dllPath) {
    Copy-Item $dllPath -Destination $portableDir
  } else {
    Write-Host "Warning: Required DLL not found: $dll"
  }
}

if (Test-Path $portableZip) {
  Remove-Item -Force $portableZip
}
Compress-Archive -Path (Join-Path $portableDir '*') -DestinationPath $portableZip -CompressionLevel Optimal

if (-not (Test-Path $portableZip)) {
  throw "Portable zip not found at $portableZip"
}

Write-Host "Portable directory produced: $portableDir"
Write-Host "Portable zip produced: $portableZip"

if ($Sign) {
  if (-not $env:SIGN_CERT_PATH) { Throw "SIGN_CERT_PATH environment variable required to sign" }
  Write-Host "Signing MSI..."
  & signtool sign /f $env:SIGN_CERT_PATH /p $env:SIGN_CERT_PASS /fd SHA256 /tr http://timestamp.digicert.com /td SHA256 $msi
}

Write-Host "Build complete - outputs are in: $outputDir"
Pop-Location
