#!/usr/bin/env pwsh
# Build script for Smart Video Compressor with Tauri frontend

param(
    [switch]$Release = $false
)

$ErrorActionPreference = "Stop"

Write-Host ""
Write-Host "================================================" -ForegroundColor Cyan
Write-Host "Smart Video Compressor - Tauri Build" -ForegroundColor Cyan
Write-Host "================================================" -ForegroundColor Cyan
Write-Host ""

# Determine configuration
$config = if ($Release) { "Release" } else { "Debug" }
Write-Host "Build Configuration: $config" -ForegroundColor Yellow
Write-Host ""

# Step 1: Build .NET Backend
Write-Host "Step 1: Building .NET backend..." -ForegroundColor Green
Write-Host "---------------------------------------------"

$backendOutputDir = "backend-build"

try {
    # Build for Windows x64
    Write-Host "Building backend for Windows x64..." -ForegroundColor Cyan
    dotnet publish -c $config -r win-x64 -o $backendOutputDir --self-contained true /p:PublishSingleFile=true
    
    if ($LASTEXITCODE -ne 0) {
        throw "Backend build failed with exit code $LASTEXITCODE"
    }
    
    Write-Host "✓ Backend build successful" -ForegroundColor Green
    Write-Host ""
}
catch {
    Write-Host "✗ Backend build failed: $_" -ForegroundColor Red
    exit 1
}

# Step 2: Copy backend to Tauri sidecar location
Write-Host "Step 2: Preparing Tauri sidecar binaries..." -ForegroundColor Green
Write-Host "---------------------------------------------"

try {
    $tauriSidecarDir = "tauri/src-tauri/binaries"
    
    # Create sidecar directory if it doesn't exist
    if (-not (Test-Path $tauriSidecarDir)) {
        New-Item -ItemType Directory -Path $tauriSidecarDir | Out-Null
    }
    
    # Copy backend executable with platform-specific naming
    # Tauri expects sidecars named: <name>-<target-triple>.exe
    $backendExe = Join-Path $backendOutputDir "smart-compressor.exe"
    $sidecarExe = Join-Path $tauriSidecarDir "smart-compressor-backend-x86_64-pc-windows-msvc.exe"
    
    if (Test-Path $backendExe) {
        Copy-Item $backendExe $sidecarExe -Force
        Write-Host "✓ Copied backend to: $sidecarExe" -ForegroundColor Green
    }
    else {
        throw "Backend executable not found at: $backendExe"
    }
    
    Write-Host ""
}
catch {
    Write-Host "✗ Sidecar preparation failed: $_" -ForegroundColor Red
    exit 1
}

# Step 3: Build Tauri application
Write-Host "Step 3: Building Tauri application..." -ForegroundColor Green
Write-Host "---------------------------------------------"

try {
    Push-Location tauri
    
    # Install npm dependencies if needed
    if (-not (Test-Path "node_modules")) {
        Write-Host "Installing npm dependencies..." -ForegroundColor Cyan
        npm install
        if ($LASTEXITCODE -ne 0) {
            throw "npm install failed"
        }
    }
    
    # Build Tauri app
    if ($Release) {
        Write-Host "Building Tauri app (Release)..." -ForegroundColor Cyan
        npm run tauri build
    }
    else {
        Write-Host "Building Tauri app (Debug)..." -ForegroundColor Cyan
        npm run tauri build
    }
    
    if ($LASTEXITCODE -ne 0) {
        throw "Tauri build failed with exit code $LASTEXITCODE"
    }
    
    Pop-Location
    
    Write-Host "✓ Tauri build successful" -ForegroundColor Green
    Write-Host ""
}
catch {
    Pop-Location
    Write-Host "✗ Tauri build failed: $_" -ForegroundColor Red
    exit 1
}

# Step 4: Copy final executable to publish-win
Write-Host "Step 4: Copying final executable to publish-win..." -ForegroundColor Green
Write-Host "---------------------------------------------"

try {
    $publishDir = "publish-win"
    
    # Create publish-win directory if it doesn't exist
    if (-not (Test-Path $publishDir)) {
        New-Item -ItemType Directory -Path $publishDir | Out-Null
    }
    
    if ($Release) {
        $bundleDir = "tauri/src-tauri/target/release/bundle"
        
        # Find and copy the NSIS or MSI installer
        $msiPath = Get-ChildItem -Path "$bundleDir/msi" -Filter "*.exe" -ErrorAction SilentlyContinue | Select-Object -First 1
        $nsisPath = Get-ChildItem -Path "$bundleDir/nsis" -Filter "*.exe" -ErrorAction SilentlyContinue | Select-Object -First 1
        
        if ($msiPath) {
            Copy-Item -Path $msiPath.FullName -Destination "$publishDir/smart-compressor.exe" -Force
            Write-Host "✓ Copied MSI installer to: $publishDir/smart-compressor.exe" -ForegroundColor Green
        }
        elseif ($nsisPath) {
            Copy-Item -Path $nsisPath.FullName -Destination "$publishDir/smart-compressor.exe" -Force
            Write-Host "✓ Copied NSIS installer to: $publishDir/smart-compressor.exe" -ForegroundColor Green
        }
        else {
            Write-Host "⚠ No installer found, copying sidecar binary instead" -ForegroundColor Yellow
            $backendExe = "tauri/src-tauri/target/release/smart-compressor-backend.exe"
            if (Test-Path $backendExe) {
                Copy-Item -Path $backendExe -Destination "$publishDir/smart-compressor.exe" -Force
                Write-Host "✓ Copied backend to: $publishDir/smart-compressor.exe" -ForegroundColor Green
            }
        }
    }
    else {
        $debugExe = "tauri/src-tauri/target/debug/smart-compressor.exe"
        if (Test-Path $debugExe) {
            Copy-Item -Path $debugExe -Destination "$publishDir/smart-compressor-debug.exe" -Force
            Write-Host "✓ Copied debug executable to: $publishDir/smart-compressor-debug.exe" -ForegroundColor Green
        }
    }
    
    Write-Host ""
}
catch {
    Write-Host "⚠ Warning: Could not copy to publish-win: $_" -ForegroundColor Yellow
    Write-Host ""
}

# Step 5: Show output location
Write-Host "================================================" -ForegroundColor Cyan
Write-Host "Build Complete!" -ForegroundColor Green
Write-Host "================================================" -ForegroundColor Cyan
Write-Host ""

$publishExe = "$publishDir/smart-compressor.exe"
if (Test-Path $publishExe) {
    $fileSize = (Get-Item $publishExe).Length / 1MB
    Write-Host "Final executable: $publishExe" -ForegroundColor Yellow
    Write-Host "File size: $([Math]::Round($fileSize, 2)) MB" -ForegroundColor Yellow
}
else {
    if ($Release) {
        $bundleDir = "tauri/src-tauri/target/release/bundle"
        Write-Host "Build artifacts: $bundleDir" -ForegroundColor Yellow
        Write-Host ""
        
        if (Test-Path $bundleDir) {
            Write-Host "Available bundles:" -ForegroundColor Cyan
            Get-ChildItem $bundleDir -Directory | ForEach-Object {
                Write-Host "  - $($_.Name)" -ForegroundColor White
            }
        }
    }
    else {
        Write-Host "Debug build output: tauri/src-tauri/target/debug/" -ForegroundColor Yellow
    }
}

Write-Host ""
Write-Host "To run the app:" -ForegroundColor Cyan
if ($Release -and (Test-Path $publishExe)) {
    Write-Host "  $publishExe" -ForegroundColor White
}
else {
    Write-Host "  cd tauri" -ForegroundColor White
    Write-Host "  npm run tauri dev" -ForegroundColor White
}
Write-Host ""
