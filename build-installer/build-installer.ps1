param(
    [switch]$IncludeFFmpeg,
    [string]$FFmpegPath = ""
)

# PowerShell script to build the LiteClip installer using Inno Setup

$ErrorActionPreference = "Stop"
$ScriptDir = $PSScriptRoot
$RootDir = Split-Path $ScriptDir -Parent

$PublishScript = Join-Path $RootDir "publish-win.ps1"
$IssFile = Join-Path $ScriptDir "LiteClip.iss"

# 1. Check for Inno Setup Compiler (ISCC.exe)
$InnoSetupPath = "C:\Program Files (x86)\Inno Setup 6\ISCC.exe"
if (-not (Test-Path $InnoSetupPath)) {
    # Try to find it in PATH
    if (Get-Command "ISCC.exe" -ErrorAction SilentlyContinue) {
        $InnoSetupPath = "ISCC.exe"
    } else {
        Write-Host "ERROR: Inno Setup Compiler (ISCC.exe) not found." -ForegroundColor Red
        Write-Host "Please install Inno Setup 6 from https://jrsoftware.org/isdl.php" -ForegroundColor Yellow
        exit 1
    }
}

Write-Host "Found Inno Setup Compiler: $InnoSetupPath" -ForegroundColor Green

# 2. Run the publish script to generate the binaries
Write-Host "Building and publishing application..." -ForegroundColor Cyan
Push-Location $RootDir
try {
    if ($IncludeFFmpeg) {
        Write-Host "Publishing with FFmpeg included. FFmpegPath = $FFmpegPath" -ForegroundColor Cyan
        & $PublishScript -IncludeFFmpeg -FFmpegPath $FFmpegPath
    } else {
        & $PublishScript
    }
    if ($LASTEXITCODE -ne 0) { throw "Publish script failed" }
} catch {
    Write-Host "ERROR: Failed to run publish script: $_" -ForegroundColor Red
    exit 1
}
finally {
    Pop-Location
}

# 3. Compile the installer
Write-Host "Compiling installer..." -ForegroundColor Cyan
try {
    & $InnoSetupPath $IssFile
    if ($LASTEXITCODE -ne 0) { throw "Inno Setup compilation failed" }
} catch {
    Write-Host "ERROR: Failed to compile installer: $_" -ForegroundColor Red
    exit 1
}

Write-Host ""
Write-Host "=== Installer Build Complete ===" -ForegroundColor Green
Write-Host "Installer location: $(Join-Path $RootDir 'dist')" -ForegroundColor Cyan
