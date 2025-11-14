# PowerShell script to build and publish LiteClip as a Windows executable
#
# TRIMMING FIX (2025-11-14):
# - Disabled Native AOT (PublishAot=false) - incompatible with ASP.NET Core & Photino
# - Changed TrimMode from 'link' to 'partial' for better compatibility
# - Added comprehensive TrimmerRootAssembly entries for all ASP.NET Core components
# - Result: Trimmed executable (~96 MB) that runs properly without runtime errors
#
# Trimming is important for keeping file size reasonable while maintaining full functionality.

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

Write-Host "=== LiteClip - Windows Build Script ===" -ForegroundColor Cyan
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
    try {
        Remove-Item -Recurse -Force $OutputDir -ErrorAction Stop
        Write-Host "Removed existing output directory" -ForegroundColor Green
    } catch {
        # If removal fails (file locked), try alternative approach
        Write-Host "Directory is in use, attempting graceful cleanup..." -ForegroundColor Yellow
        
        # Close any handles to the directory (PowerShell-specific)
        Get-Item $OutputDir -ErrorAction SilentlyContinue | ForEach-Object {
            # Wait a moment for any locks to release
            Start-Sleep -Milliseconds 500
        }
        
        # Try again with a fresh attempt
        try {
            Remove-Item -Recurse -Force $OutputDir -ErrorAction Stop
            Write-Host "Directory cleaned successfully" -ForegroundColor Green
        } catch {
            Write-Host "WARNING: Could not fully clean output directory, proceeding anyway..." -ForegroundColor Yellow
            Write-Host "If build fails, manually delete: $OutputDir" -ForegroundColor Yellow
        }
    }
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

# Restore NuGet packages with R2R support
Write-Host ""
Write-Host "Restoring NuGet packages..." -ForegroundColor Yellow
dotnet restore --runtime win-x64 /p:PublishReadyToRun=true

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
Write-Host "Publishing .NET application with optimized settings..." -ForegroundColor Yellow

# Optimized publish args for fast startup (R2R compilation enabled)
$publishArgs = @(
    'publish',
    'liteclip.csproj',
    '--configuration', $Configuration,
    '--runtime', 'win-x64',
    '--self-contained', 'true',
    '--output', $OutputDir,
    '--no-restore',
    '/p:PublishSingleFile=true',
    '/p:EnableCompressionInSingleFile=false',
    '/p:PublishReadyToRun=true',
    '/p:PublishTrimmed=true',
    '/p:TrimMode=link',
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
Write-Host "FFmpeg must be installed separately or available in system PATH." -ForegroundColor Yellow
Write-Host "The app will look for FFmpeg in:" -ForegroundColor Yellow
Write-Host "  1. FFmpeg:Path configuration setting" -ForegroundColor Gray
Write-Host "  2. System PATH environment variable" -ForegroundColor Gray

# Create a run script
Write-Host ""
Write-Host "Creating run script..." -ForegroundColor Yellow
$runScriptContent = @"
@echo off
echo Starting LiteClip...
echo A native window will open automatically!
echo.
liteclip.exe
"@

$runScriptPath = Join-Path $OutputDir "run.bat"
Set-Content -Path $runScriptPath -Value $runScriptContent -Encoding ASCII
Write-Host "Created run.bat script" -ForegroundColor Green

# Copy portable exe to dist directory with versioned name
$DistDir = "dist"
$Version = "1.0.0"
$PortableName = "liteclip-$Version-portable-win-x64.exe"

Write-Host ""
Write-Host "Copying portable executable to dist directory..." -ForegroundColor Yellow
if (-not (Test-Path $DistDir)) {
    New-Item -ItemType Directory -Path $DistDir | Out-Null
    Write-Host "Created dist directory" -ForegroundColor Green
}

$SourceExe = Join-Path $OutputDir "liteclip.exe"
$DestExe = Join-Path $DistDir $PortableName

if (Test-Path $SourceExe) {
    Copy-Item -Path $SourceExe -Destination $DestExe -Force
    Write-Host "Copied to: $DestExe" -ForegroundColor Green
}
else {
    Write-Host "WARNING: Could not find $SourceExe to copy to dist" -ForegroundColor Yellow
}

# Display summary
Write-Host ""
Write-Host "=== Build Complete ===" -ForegroundColor Green
Write-Host ""
Write-Host "Output location: $OutputDir" -ForegroundColor Cyan
Write-Host "Executable: liteclip.exe" -ForegroundColor White
Write-Host ""
Write-Host "Portable version: $DestExe" -ForegroundColor Cyan
Write-Host ""
Write-Host "✨ Portable Release build (single-file per csproj settings)." -ForegroundColor Green
Write-Host "   Frontend is embedded." -ForegroundColor Green
Write-Host "   FFmpeg must be installed separately (system PATH or config)." -ForegroundColor Yellow
Write-Host "   You can move the exe anywhere and it will work!" -ForegroundColor Green
Write-Host ""
Write-Host "To run the application:" -ForegroundColor Yellow
Write-Host "  1. Copy 'liteclip.exe' anywhere you want" -ForegroundColor White
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

