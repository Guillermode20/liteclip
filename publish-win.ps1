# PowerShell script to build and publish smart-compressor as a Windows executable

param(
    [string]$Configuration = "Release",
    [string]$OutputDir = "publish-win",
    [switch]$IncludeFFmpeg,
    [string]$FFmpegPath = ""
)

Write-Host "=== Smart Video Compressor - Windows Build Script ===" -ForegroundColor Cyan
Write-Host ""

# Check if .NET SDK is available
Write-Host "Checking .NET SDK..." -ForegroundColor Yellow
try {
    $dotnetVersion = dotnet --version
    Write-Host "Found .NET SDK version: $dotnetVersion" -ForegroundColor Green
} catch {
    Write-Host "ERROR: .NET SDK not found. Please install .NET 9.0 SDK or later." -ForegroundColor Red
    exit 1
}

# Check if Node.js is available
Write-Host "Checking Node.js..." -ForegroundColor Yellow
try {
    $nodeVersion = node --version
    Write-Host "Found Node.js version: $nodeVersion" -ForegroundColor Green
} catch {
    Write-Host "ERROR: Node.js not found. Please install Node.js to build the frontend." -ForegroundColor Red
    exit 1
}

# Clean previous builds
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
    exit 1
} finally {
    Pop-Location
}

# Publish .NET application
Write-Host ""
Write-Host "Publishing .NET application..." -ForegroundColor Yellow
Write-Host "Configuration: $Configuration" -ForegroundColor Cyan
Write-Host "Output directory: $OutputDir" -ForegroundColor Cyan

$publishArgs = @(
    "publish",
    "--configuration", $Configuration,
    "--runtime", "win-x64",
    "--self-contained", "true",
    "--output", $OutputDir,
    "/p:PublishSingleFile=true",
    "/p:IncludeNativeLibrariesForSelfExtract=true",
    "/p:EnableCompressionInSingleFile=true",
    "/p:PublishReadyToRun=true"
)

dotnet @publishArgs

if ($LASTEXITCODE -ne 0) {
    Write-Host "ERROR: .NET publish failed" -ForegroundColor Red
    exit 1
}

Write-Host ".NET application published successfully" -ForegroundColor Green

# FFmpeg is now embedded in the exe!
Write-Host ""
Write-Host "=== FFmpeg Status ===" -ForegroundColor Cyan
Write-Host "FFmpeg is embedded in the executable!" -ForegroundColor Green
Write-Host "The exe will automatically extract FFmpeg on first run." -ForegroundColor Green

# Create a run script
Write-Host ""
Write-Host "Creating run script..." -ForegroundColor Yellow
$runScriptContent = @"
@echo off
echo Starting Smart Video Compressor...
echo Your browser will open automatically!
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
Write-Host "✨ This is a portable single-file executable!" -ForegroundColor Green
Write-Host "   Frontend and FFmpeg are embedded." -ForegroundColor Green
Write-Host "   You can move it anywhere and it will work!" -ForegroundColor Green
Write-Host ""
Write-Host "To run the application:" -ForegroundColor Yellow
Write-Host "  1. Copy 'smart-compressor.exe' anywhere you want" -ForegroundColor White
Write-Host "  2. Double-click it or run from command line" -ForegroundColor White
Write-Host "  3. Browser will open automatically!" -ForegroundColor White
Write-Host ""

Write-Host "Press any key to exit..." -ForegroundColor Gray
$null = $Host.UI.RawUI.ReadKey("NoEcho,IncludeKeyDown")

