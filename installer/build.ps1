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

Write-Host "2) Restoring NuGet packages for WiX project"
if (-not (Get-Command heat.exe -ErrorAction SilentlyContinue)) {
  Write-Host "Warning: WiX Toolset (heat.exe) not found in PATH. If 'dotnet restore' fails, please install WiX v4 or ensure heat.exe is available. See installer/README.md"
}
# ensure NuGet packages (WixToolset.MSBuild) are restored before msbuild
dotnet restore .\LiteClipReplay.wixproj

Write-Host "3) Building WiX installer project"
dotnet msbuild .\LiteClipReplay.wixproj /t:Build /p:Configuration=$Configuration /p:Platform=$Platform /p:ProductVersion=$ProductVersion

# WiX v4 outputs to en-US subdirectory
$msi = Join-Path $PSScriptRoot "output\en-US\LiteClipReplay.msi"
if (Test-Path $msi) {
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
