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

# Copy or remind about FFmpeg
Write-Host ""
Write-Host "=== FFmpeg Setup ===" -ForegroundColor Cyan

if ($IncludeFFmpeg -and $FFmpegPath -ne "" -and (Test-Path $FFmpegPath)) {
    Write-Host "Copying FFmpeg from: $FFmpegPath" -ForegroundColor Yellow
    $ffmpegDir = Join-Path $OutputDir "ffmpeg"
    New-Item -ItemType Directory -Force -Path $ffmpegDir | Out-Null
    Copy-Item $FFmpegPath (Join-Path $ffmpegDir "ffmpeg.exe")
    Write-Host "FFmpeg copied to output directory" -ForegroundColor Green
} elseif ($IncludeFFmpeg) {
    Write-Host "WARNING: -IncludeFFmpeg specified but FFmpegPath not provided or invalid" -ForegroundColor Yellow
    Write-Host "Please manually copy ffmpeg.exe to: $OutputDir\ffmpeg\" -ForegroundColor Yellow
} else {
    Write-Host "FFmpeg not included in the build." -ForegroundColor Yellow
    Write-Host ""
    Write-Host "To include FFmpeg in your distribution:" -ForegroundColor Cyan
    Write-Host "  1. Download FFmpeg from: https://www.gyan.dev/ffmpeg/builds/" -ForegroundColor White
    Write-Host "  2. Extract ffmpeg.exe" -ForegroundColor White
    Write-Host "  3. Create a 'ffmpeg' folder in the output directory: $OutputDir\ffmpeg\" -ForegroundColor White
    Write-Host "  4. Copy ffmpeg.exe to that folder" -ForegroundColor White
    Write-Host ""
    Write-Host "Or run this script again with: -IncludeFFmpeg -FFmpegPath ""C:\path\to\ffmpeg.exe""" -ForegroundColor White
}

# Create a run script
Write-Host ""
Write-Host "Creating run script..." -ForegroundColor Yellow
$runScriptContent = @"
@echo off
echo Starting Smart Video Compressor...
echo.
echo The application will be available at: http://localhost:5000
echo Press Ctrl+C to stop the server
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
Write-Host "To run the application:" -ForegroundColor Yellow
Write-Host "  1. Navigate to: $OutputDir" -ForegroundColor White
Write-Host "  2. Double-click 'run.bat' or run 'smart-compressor.exe'" -ForegroundColor White
Write-Host "  3. Open browser to: http://localhost:5000" -ForegroundColor White
Write-Host ""

if (-not $IncludeFFmpeg -or $FFmpegPath -eq "") {
    Write-Host "IMPORTANT: Don't forget to add FFmpeg!" -ForegroundColor Red
}

Write-Host "Press any key to exit..." -ForegroundColor Gray
$null = $Host.UI.RawUI.ReadKey("NoEcho,IncludeKeyDown")

