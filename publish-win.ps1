# PowerShell script to build and publish smart-compressor as a Windows executable

param(
    [string]$Configuration = 'Release',
    [string]$OutputDir = 'publish-win',
    [switch]$IncludeFFmpeg,
    [string]$FFmpegPath = ""
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

Write-Host "=== Smart Video Compressor - Windows Build Script ===" -ForegroundColor Cyan
Write-Host ""
Write-Host "Configuration: $Configuration" -ForegroundColor Cyan
Write-Host "Output directory: $OutputDir" -ForegroundColor Cyan

# Check if .NET SDK is available
Write-Host "Checking .NET SDK..." -ForegroundColor Yellow
try {
    $dotnetVersion = dotnet --version
    Write-Host "Found .NET SDK version: $dotnetVersion" -ForegroundColor Green
} catch {
    Write-Host "ERROR: .NET SDK not found. Please install .NET 9.0 SDK or later." -ForegroundColor Red
    Write-Host ""
    Write-Host "Press any key to exit..." -ForegroundColor Gray
    $null = $Host.UI.RawUI.ReadKey("NoEcho,IncludeKeyDown")
    exit 1
}

# Check if Node.js is available
Write-Host "Checking Node.js..." -ForegroundColor Yellow
try {
    $nodeVersion = node --version
    Write-Host "Found Node.js version: $nodeVersion" -ForegroundColor Green
} catch {
    Write-Host "ERROR: Node.js not found. Please install Node.js to build the frontend." -ForegroundColor Red
    Write-Host ""
    Write-Host "Press any key to exit..." -ForegroundColor Gray
    $null = $Host.UI.RawUI.ReadKey("NoEcho,IncludeKeyDown")
    exit 1
}

# Clean previous builds (always for Release)
Write-Host ""
Write-Host "Cleaning previous builds..." -ForegroundColor Yellow
if (Test-Path $OutputDir) {
    Remove-Item -Recurse -Force $OutputDir
    Write-Host "Removed existing output directory" -ForegroundColor Green
}

# Build frontend
Write-Host ""
Write-Host "Building frontend..." -ForegroundColor Yellow
Push-Location frontend
try {
    # Install dependencies if node_modules doesn't exist
    if (-not (Test-Path "node_modules")) {
        Write-Host "Installing frontend dependencies..." -ForegroundColor Yellow
        npm install
        if ($LASTEXITCODE -ne 0) {
            throw "npm install failed"
        }
    }
    
    # Build frontend
    Write-Host "Running frontend build..." -ForegroundColor Yellow
    npm run build
    if ($LASTEXITCODE -ne 0) {
        throw "Frontend build failed"
    }
    Write-Host "Frontend built successfully" -ForegroundColor Green
} catch {
    Write-Host "ERROR: Frontend build failed: $_" -ForegroundColor Red
    Pop-Location
    Write-Host ""
    Write-Host "Press any key to exit..." -ForegroundColor Gray
    $null = $Host.UI.RawUI.ReadKey("NoEcho,IncludeKeyDown")
    exit 1
} finally {
    Pop-Location
}

# Restore NuGet packages
Write-Host ""
Write-Host "Restoring NuGet packages..." -ForegroundColor Yellow
dotnet restore

if ($LASTEXITCODE -ne 0) {
    Write-Host ""
    Write-Host "ERROR: NuGet restore failed" -ForegroundColor Red
    Write-Host ""
    Write-Host "Press any key to exit..." -ForegroundColor Gray
    $null = $Host.UI.RawUI.ReadKey("NoEcho,IncludeKeyDown")
    exit 1
}

Write-Host "Packages restored successfully" -ForegroundColor Green

# Publish .NET application
Write-Host ""
Write-Host "Publishing .NET application..." -ForegroundColor Yellow

# Let csproj control single-file, self-contained, R2R, compression, etc.
$publishArgs = @(
    'publish',
    '--configuration', $Configuration,
    '--runtime', 'win-x64',
    '--output', $OutputDir,
    '--no-restore',
    '/maxcpucount'
)

$sw = [System.Diagnostics.Stopwatch]::StartNew()
dotnet @publishArgs
$sw.Stop()

if ($LASTEXITCODE -ne 0) {
    Write-Host ""
    Write-Host "ERROR: .NET publish failed" -ForegroundColor Red
    Write-Host ""
    Write-Host "Press any key to exit..." -ForegroundColor Gray
    $null = $Host.UI.RawUI.ReadKey("NoEcho,IncludeKeyDown")
    exit 1
}

Write-Host ".NET application published successfully" -ForegroundColor Green
Write-Host ("Publish duration: {0:mm\:ss\.fff}" -f $sw.Elapsed) -ForegroundColor Cyan

# FFmpeg status (Release)
Write-Host ""
Write-Host "=== FFmpeg Status ===" -ForegroundColor Cyan
Write-Host "FFmpeg is embedded in the executable (Release)." -ForegroundColor Green
Write-Host "The exe may extract resources on first run if needed." -ForegroundColor Green

# Create a run script
Write-Host ""
Write-Host "Creating run script..." -ForegroundColor Yellow
$runScriptContent = @"
@echo off
echo Starting Smart Video Compressor...
echo A native window will open automatically!
echo.
smart-compressor.exe --urls "http://localhost:5000"
"@

$runScriptPath = Join-Path $OutputDir "run.bat"
Set-Content -Path $runScriptPath -Value $runScriptContent -Encoding ASCII
Write-Host "Created run.bat script" -ForegroundColor Green

# Display summary
Write-Host ""
Write-Host "=== Build Complete ===" -ForegroundColor Green
Write-Host ""
Write-Host "Output location: $OutputDir" -ForegroundColor Cyan
Write-Host "Executable: smart-compressor.exe" -ForegroundColor White
Write-Host ""
Write-Host "✨ Portable Release build (single-file per csproj settings)." -ForegroundColor Green
Write-Host "   Frontend and FFmpeg are embedded." -ForegroundColor Green
Write-Host "   You can move it anywhere and it will work!" -ForegroundColor Green
Write-Host ""
Write-Host "To run the application:" -ForegroundColor Yellow
Write-Host "  1. Copy 'smart-compressor.exe' anywhere you want" -ForegroundColor White
Write-Host "  2. Double-click it or run from command line" -ForegroundColor White
Write-Host "  3. A native desktop window will open automatically!" -ForegroundColor White
Write-Host ""
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

