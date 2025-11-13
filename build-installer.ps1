# PowerShell script to build LiteClip Windows installer using Inno Setup
# Version: 1.0.0

param(
    [string]$InnoSetupPath = ""
)

# Set error action preference to stop on errors
$ErrorActionPreference = "Stop"

# Trap any unhandled errors
trap {
    Write-Host ""
    Write-Host "========================================" -ForegroundColor Red
    Write-Host "ERROR: An unexpected error occurred!" -ForegroundColor Red
    Write-Host "========================================" -ForegroundColor Red
    Write-Host $_.Exception.Message -ForegroundColor Red
    Write-Host ""
    Write-Host "Press any key to exit..." -ForegroundColor Gray
    $null = $Host.UI.RawUI.ReadKey("NoEcho,IncludeKeyDown")
    exit 1
}

$VERSION = "1.0.0"
$ISS_FILE = "liteclip-installer.iss"
$DIST_DIR = "dist"

Write-Host "=== LiteClip - Windows Installer Build Script ===" -ForegroundColor Cyan
Write-Host ""
Write-Host "Version: $VERSION" -ForegroundColor Cyan
Write-Host ""

# Step 1: Build the portable executable first
Write-Host "Step 1: Building portable executable..." -ForegroundColor Yellow
Write-Host ""

& .\publish-win.ps1

if ($LASTEXITCODE -ne 0) {
    Write-Host ""
    Write-Host "ERROR: Portable build failed" -ForegroundColor Red
    Write-Host ""
    Write-Host "Press any key to exit..." -ForegroundColor Gray
    $null = $Host.UI.RawUI.ReadKey("NoEcho,IncludeKeyDown")
    exit 1
}

# Step 2: Find Inno Setup Compiler
Write-Host ""
Write-Host "Step 2: Locating Inno Setup Compiler..." -ForegroundColor Yellow

$IsccPath = $null

# Check if user provided a path
if ($InnoSetupPath -ne "" -and (Test-Path $InnoSetupPath)) {
    $IsccPath = $InnoSetupPath
    Write-Host "Using user-provided Inno Setup path: $IsccPath" -ForegroundColor Green
}
else {
    # Common installation paths for Inno Setup
    $CommonPaths = @(
        "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
        "${env:ProgramFiles}\Inno Setup 6\ISCC.exe",
        "${env:ProgramFiles(x86)}\Inno Setup 5\ISCC.exe",
        "${env:ProgramFiles}\Inno Setup 5\ISCC.exe"
    )

    foreach ($path in $CommonPaths) {
        if (Test-Path $path) {
            $IsccPath = $path
            Write-Host "Found Inno Setup at: $IsccPath" -ForegroundColor Green
            break
        }
    }
}

if ($null -eq $IsccPath) {
    Write-Host ""
    Write-Host "========================================" -ForegroundColor Red
    Write-Host "ERROR: Inno Setup Compiler not found!" -ForegroundColor Red
    Write-Host "========================================" -ForegroundColor Red
    Write-Host ""
    Write-Host "Please install Inno Setup 6 from:" -ForegroundColor Yellow
    Write-Host "https://jrsoftware.org/isdl.php" -ForegroundColor Cyan
    Write-Host ""
    Write-Host "Or specify the path manually:" -ForegroundColor Yellow
    Write-Host '.\build-installer.ps1 -InnoSetupPath "C:\Path\To\ISCC.exe"' -ForegroundColor Gray
    Write-Host ""
    Write-Host "Press any key to exit..." -ForegroundColor Gray
    $null = $Host.UI.RawUI.ReadKey("NoEcho,IncludeKeyDown")
    exit 1
}

# Step 3: Create dist directory
Write-Host ""
Write-Host "Step 3: Preparing output directory..." -ForegroundColor Yellow
if (-not (Test-Path $DIST_DIR)) {
    New-Item -ItemType Directory -Path $DIST_DIR | Out-Null
    Write-Host "Created dist directory" -ForegroundColor Green
}
else {
    Write-Host "dist directory already exists" -ForegroundColor Green
}

# Step 4: Compile the installer
Write-Host ""
Write-Host "Step 4: Compiling installer with Inno Setup..." -ForegroundColor Yellow
Write-Host ""

$sw = [System.Diagnostics.Stopwatch]::StartNew()
& $IsccPath $ISS_FILE
$sw.Stop()

if ($LASTEXITCODE -ne 0) {
    Write-Host ""
    Write-Host "ERROR: Inno Setup compilation failed" -ForegroundColor Red
    Write-Host ""
    Write-Host "Press any key to exit..." -ForegroundColor Gray
    $null = $Host.UI.RawUI.ReadKey("NoEcho,IncludeKeyDown")
    exit 1
}

Write-Host ""
Write-Host "Installer compiled successfully" -ForegroundColor Green
Write-Host ("Compilation duration: {0:mm\:ss\.fff}" -f $sw.Elapsed) -ForegroundColor Cyan

# Display summary
Write-Host ""
Write-Host "=== Installer Build Complete ===" -ForegroundColor Green
Write-Host ""
Write-Host "Output location: $DIST_DIR\LiteClip-Setup-$VERSION.exe" -ForegroundColor Cyan
Write-Host ""
Write-Host "✨ Windows installer created successfully!" -ForegroundColor Green
Write-Host "   The installer will:" -ForegroundColor Green
Write-Host "   - Install LiteClip to Program Files" -ForegroundColor White
Write-Host "   - Create Start Menu shortcuts" -ForegroundColor White
Write-Host "   - Provide uninstaller" -ForegroundColor White
Write-Host "   - Show FFmpeg installation reminder" -ForegroundColor White
Write-Host ""
Write-Host "Note: Users must install FFmpeg separately" -ForegroundColor Yellow
Write-Host "Note: WebView2 Runtime required (pre-installed on Windows 10/11)" -ForegroundColor Cyan
Write-Host ""
Write-Host "========================================" -ForegroundColor Green
Write-Host "Press any key to exit..." -ForegroundColor Yellow
Write-Host "========================================" -ForegroundColor Green

# Use Read-Host as fallback if RawUI.ReadKey doesn't work
try {
    $null = $Host.UI.RawUI.ReadKey("NoEcho,IncludeKeyDown")
} catch {
    Read-Host "Press Enter to exit"
}

