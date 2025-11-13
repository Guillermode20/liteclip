#!/bin/bash

# PowerShell script to build and publish LiteClip for Linux
# Version: 1.0.0

set -e  # Exit on any error

CONFIGURATION="Release"
OUTPUT_DIR="publish-linux"
VERSION="1.0.0"

echo "=== LiteClip - Linux Build Script ==="
echo ""
echo "Configuration: $CONFIGURATION"
echo "Output directory: $OUTPUT_DIR"
echo "Version: $VERSION"
echo ""

# Check if .NET SDK is available
echo "Checking .NET SDK..."
if ! command -v dotnet &> /dev/null; then
    echo "ERROR: .NET SDK not found. Please install .NET 9.0 SDK or later."
    exit 1
fi

DOTNET_VERSION=$(dotnet --version)
echo "Found .NET SDK version: $DOTNET_VERSION"

# Check if Node.js is available
echo "Checking Node.js..."
if ! command -v node &> /dev/null; then
    echo "ERROR: Node.js not found. Please install Node.js to build the frontend."
    exit 1
fi

NODE_VERSION=$(node --version)
echo "Found Node.js version: $NODE_VERSION"

# Clean previous builds
echo ""
echo "Cleaning previous builds..."
if [ -d "$OUTPUT_DIR" ]; then
    rm -rf "$OUTPUT_DIR"
    echo "Removed existing output directory"
fi

# Build frontend
echo ""
echo "Building frontend..."
cd frontend

# Install dependencies if node_modules doesn't exist
if [ ! -d "node_modules" ]; then
    echo "Installing frontend dependencies..."
    npm install
fi

# Build frontend
echo "Running frontend build..."
npm run build

echo "Frontend built successfully"
cd ..

# Restore NuGet packages
echo ""
echo "Restoring NuGet packages..."
dotnet restore --runtime linux-x64

echo "Packages restored successfully"

# Publish .NET application
echo ""
echo "Publishing .NET application with optimized settings..."

# Optimized publish args for fast startup (no compression, no R2R)
START_TIME=$(date +%s)

dotnet publish liteclip.csproj \
    --configuration "$CONFIGURATION" \
    --runtime linux-x64 \
    --self-contained true \
    --output "$OUTPUT_DIR" \
    --no-restore \
    /p:PublishSingleFile=true \
    /p:EnableCompressionInSingleFile=false \
    /p:PublishReadyToRun=false \
    /maxcpucount

END_TIME=$(date +%s)
DURATION=$((END_TIME - START_TIME))

echo ".NET application published successfully"
echo "Publish duration: ${DURATION} seconds"

# FFmpeg status
echo ""
echo "=== FFmpeg Status ==="
echo "FFmpeg must be installed separately or available in system PATH."
echo "The app will look for FFmpeg in:"
echo "  1. FFmpeg:Path configuration setting"
echo "  2. System PATH environment variable"
echo ""
echo "Install FFmpeg on Fedora: sudo dnf install ffmpeg"

# Display summary
echo ""
echo "=== Build Complete ==="
echo ""
echo "Output location: $OUTPUT_DIR"
echo "Executable: liteclip"
echo ""
echo "✨ Portable Release build (single-file per csproj settings)."
echo "   Frontend is embedded."
echo "   FFmpeg must be installed separately (system PATH or config)."
echo "   You can move the binary anywhere and it will work!"
echo ""
echo "To run the application:"
echo "  1. Make sure webkit2gtk4.0 is installed: sudo dnf install webkit2gtk4.0"
echo "  2. Make sure FFmpeg is installed: sudo dnf install ffmpeg"
echo "  3. Run: ./$OUTPUT_DIR/liteclip"
echo ""
echo "Note: webkit2gtk4.0 required for Photino.NET on Linux"
echo ""

