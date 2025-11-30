# PowerShell script to build and publish LiteClip as a Windows executable
#
# TRIMMING FIX (2025-11-26):
# - Disabled Native AOT (PublishAot=false) - incompatible with ASP.NET Core & Photino
# - Disabled PublishTrimmed entirely - TrimMode=link breaks ManifestEmbeddedFileProvider
#   which is required to serve the embedded wwwroot static files in Release builds.
#   With trimming enabled, the app shows a black window because static files fail to load.
# - R2R (ReadyToRun) is still enabled for fast startup without breaking embedded resources.

param(
    [string]$Configuration = 'Release',
    [string]$OutputDir = 'publish-win',
    [switch]$IncludeFFmpeg,
    [string]$FFmpegPath = "",
    [switch]$SkipPause
)

if ($env:CI -eq 'true') {
    $SkipPause = $true
}

function Invoke-PauseIfNeeded {
    param(
        [string]$Message = "Press any key to exit..."
    )

    if ($SkipPause) {
        return
    }

    Write-Host $Message -ForegroundColor Gray
    try {
        $null = $Host.UI.RawUI.ReadKey("NoEcho,IncludeKeyDown")
    } catch {
        Read-Host "Press Enter to continue"
    }
}

function Get-AppVersion {
    $assemblyInfoPath = Join-Path (Join-Path $PSScriptRoot "Properties") "AssemblyInfo.cs"
    if (-not (Test-Path $assemblyInfoPath)) {
        return "0.0.0"
    }

    $match = Select-String -Path $assemblyInfoPath -Pattern 'AssemblyInformationalVersion\("(?<ver>[^\"]+)"\)' | Select-Object -First 1
    if ($match) {
        return $match.Matches[0].Groups['ver'].Value
    }

    return "0.0.0"
}

function Get-SafeVersionString {
    param([string]$Version)

    if ([string]::IsNullOrWhiteSpace($Version)) {
        return "0.0.0"
    }

    $invalidChars = [System.IO.Path]::GetInvalidFileNameChars()
    $safe = ($Version.ToCharArray() | ForEach-Object {
            if ($invalidChars -contains $_) { '-' } else { $_ }
        }) -join ''

    if ([string]::IsNullOrWhiteSpace($safe)) {
        return "0.0.0"
    }

    return $safe
}

$AppVersion = Get-AppVersion
$SafeAppVersion = Get-SafeVersionString -Version $AppVersion
Write-Host "Application version: $AppVersion" -ForegroundColor Cyan

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
    Invoke-PauseIfNeeded
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
    Write-Host "ERROR: .NET SDK not found. Please install .NET 10.0 SDK or later." -ForegroundColor Red
    Write-Host ""
    Invoke-PauseIfNeeded
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
    Invoke-PauseIfNeeded
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
    Invoke-PauseIfNeeded
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
    Invoke-PauseIfNeeded
    exit 1
}

Write-Host "Packages restored successfully" -ForegroundColor Green

# Publish .NET application (folder-based, framework-dependent)
Write-Host "" 
Write-Host "Publishing .NET application (folder-based Release build)..." -ForegroundColor Yellow

# Folder-based publish args. Single-file + self-contained are disabled via csproj
# for this Windows Release build to keep Photino/WebView2 stable.
$publishArgs = @(
    'publish',
    'liteclip.csproj',
    '--configuration', $Configuration,
    '--runtime', 'win-x64',
    '--output', $OutputDir,
    '--no-restore',
    '/p:PublishTrimmed=false',
    '/maxcpucount'
)

$sw = [System.Diagnostics.Stopwatch]::StartNew()
dotnet @publishArgs
$sw.Stop()

if ($LASTEXITCODE -ne 0) {
    Write-Host ""
    Write-Host "ERROR: .NET publish failed" -ForegroundColor Red
    Write-Host ""
    Invoke-PauseIfNeeded
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

# If requested, copy the local FFmpeg binary or folder into the publish output so the
# installer (build-installer) can include it without requiring download at install time.
if ($IncludeFFmpeg) {
    Write-Host "IncludeFFmpeg requested. Attempting to locate/copy FFmpeg..." -ForegroundColor Cyan

    try {
        $ffDestDir = Join-Path $OutputDir "ffmpeg"
        New-Item -ItemType Directory -Path $ffDestDir -Force | Out-Null

        if ([string]::IsNullOrWhiteSpace($FFmpegPath)) {
            # Look up ffmpeg from PATH
            $ffCmd = Get-Command ffmpeg -ErrorAction SilentlyContinue
            if ($ffCmd) {
                $ffExe = $ffCmd.Source
                Write-Host "Found ffmpeg on PATH at: $ffExe" -ForegroundColor Green
                Copy-Item -Path $ffExe -Destination (Join-Path $ffDestDir "ffmpeg.exe") -Force
            } else {
                Write-Host "FFmpeg not found in PATH and no FFmpegPath supplied." -ForegroundColor Yellow
                Write-Host "Provide -FFmpegPath <path-to-ffmpeg.exe> or ensure ffmpeg is in PATH." -ForegroundColor Yellow
            }
        }
        else {
            $expanded = (Resolve-Path -LiteralPath $FFmpegPath -ErrorAction SilentlyContinue)
            if ($expanded) {
                $pathStr = $expanded.ToString()
                if (Test-Path $pathStr -PathType Leaf) {
                    # It's a file
                    Write-Host "Copying ffmpeg.exe from: $pathStr" -ForegroundColor Green
                    Copy-Item -Path $pathStr -Destination (Join-Path $ffDestDir "ffmpeg.exe") -Force
                } elseif (Test-Path $pathStr -PathType Container) {
                    # Provided a directory - try to find ffmpeg.exe inside
                    $exe = Get-ChildItem -Path $pathStr -Filter "ffmpeg.exe" -Recurse -File -ErrorAction SilentlyContinue | Select-Object -First 1
                    if ($exe) {
                        Write-Host "Found ffmpeg.exe in provided directory: $($exe.FullName)" -ForegroundColor Green
                        Copy-Item -Path $exe.FullName -Destination (Join-Path $ffDestDir "ffmpeg.exe") -Force
                    } else {
                        # Maybe the path points to the ffmpeg root (bin) inside the zip layout
                        $exe2 = Get-ChildItem -Path $pathStr -Filter "ffmpeg.exe" -Recurse -File -ErrorAction SilentlyContinue | Select-Object -First 1
                        if ($exe2) {
                            Copy-Item -Path $exe2.FullName -Destination (Join-Path $ffDestDir "ffmpeg.exe") -Force
                        } else {
                            Write-Host "No ffmpeg.exe found in provided directory: $pathStr" -ForegroundColor Yellow
                        }
                    }
                } else {
                    Write-Host "FFmpeg path does not exist: $FFmpegPath" -ForegroundColor Yellow
                }
            } else {
                Write-Host "Could not resolve FFmpegPath: $FFmpegPath" -ForegroundColor Yellow
            }
        }
    } catch {
        Write-Host "WARNING: Error copying FFmpeg: $_" -ForegroundColor Yellow
    }
}

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
$PortableName = "liteclip-$SafeAppVersion-portable-win-x64.exe"

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
Write-Host "Release exe copy (for convenience): $DestExe" -ForegroundColor Cyan
Write-Host "" 
Write-Host "✨ Release build (folder-based, no trimming for embedded/static file compatibility)." -ForegroundColor Green
Write-Host "   Frontend is served from wwwroot in the publish folder." -ForegroundColor Green
Write-Host "   FFmpeg must be installed separately (system PATH or config)." -ForegroundColor Yellow
Write-Host "   NOTE: liteclip.exe depends on its nearby files; keep the folder together." -ForegroundColor Yellow
Write-Host "" 
Write-Host "To run the application:" -ForegroundColor Yellow
Write-Host "  1. Open the '$OutputDir' folder" -ForegroundColor White
Write-Host "  2. Double-click 'liteclip.exe' or run 'run.bat' from command line" -ForegroundColor White
Write-Host "  3. A native desktop window will open automatically!" -ForegroundColor White
Write-Host ""
Write-Host "Note: WebView2 Runtime required (pre-installed on Windows 10/11)" -ForegroundColor Cyan
Write-Host ""

Write-Host "========================================" -ForegroundColor Green
Write-Host "Press any key to exit..." -ForegroundColor Yellow
Write-Host "========================================" -ForegroundColor Green

Invoke-PauseIfNeeded "Press any key to exit..."

