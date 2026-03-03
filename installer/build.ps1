param(
  [string]$Configuration = "Release",
  [string]$Platform = "x64",
  [string]$ProductVersion = "1.0.0.0",
  [switch]$Sign
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
Push-Location $PSScriptRoot

Write-Host "1) Building Rust release (ffmpeg feature)"
cargo build --release --features ffmpeg

if (-not (Test-Path "..\\ffmpeg\\bin\\liteclip-replay-ffmpeg.exe")) {
  throw "Missing ..\\ffmpeg\\bin\\liteclip-replay-ffmpeg.exe. Installer build requires bundled FFmpeg binaries."
}

Write-Host "2) Restoring NuGet packages for WiX project"
if (-not (Get-Command heat.exe -ErrorAction SilentlyContinue)) {
  Write-Host "Warning: WiX Toolset (heat.exe) not found in PATH. Installer will include ffmpeg.exe fallback only; full FFmpeg bin harvest is skipped."
}
# ensure NuGet packages (WixToolset.MSBuild) are restored before msbuild
dotnet restore .\LiteClipReplay.wixproj

Write-Host "3) Building WiX installer project"
$msbuildStarted = Get-Date
dotnet msbuild .\LiteClipReplay.wixproj /t:Build /p:Configuration=$Configuration /p:Platform=$Platform /p:ProductVersion=$ProductVersion
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

if ($Sign) {
  if (-not $env:SIGN_CERT_PATH) { Throw "SIGN_CERT_PATH environment variable required to sign" }
  Write-Host "Signing MSI..."
  & signtool sign /f $env:SIGN_CERT_PATH /p $env:SIGN_CERT_PASS /fd SHA256 /tr http://timestamp.digicert.com /td SHA256 $msi
}

Write-Host "Build complete - outputs are in: $PSScriptRoot\output"
Pop-Location
