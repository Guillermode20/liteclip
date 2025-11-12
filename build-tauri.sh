#!/bin/bash
# Build script for Smart Video Compressor with Tauri frontend

set -e

RELEASE=false

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --release|-r)
            RELEASE=true
            shift
            ;;
        *)
            echo "Unknown option: $1"
            exit 1
            ;;
    esac
done

echo ""
echo "================================================"
echo "Smart Video Compressor - Tauri Build"
echo "================================================"
echo ""

# Determine configuration
if [ "$RELEASE" = true ]; then
    CONFIG="Release"
else
    CONFIG="Debug"
fi

echo "Build Configuration: $CONFIG"
echo ""

# Step 1: Build .NET Backend
echo "Step 1: Building .NET backend..."
echo "---------------------------------------------"

BACKEND_OUTPUT_DIR="backend-build"

# Detect platform
if [[ "$OSTYPE" == "darwin"* ]]; then
    PLATFORM="osx"
    RID="osx-x64"
    SIDECAR_SUFFIX="x86_64-apple-darwin"
elif [[ "$OSTYPE" == "linux-gnu"* ]]; then
    PLATFORM="linux"
    RID="linux-x64"
    SIDECAR_SUFFIX="x86_64-unknown-linux-gnu"
else
    echo "Unsupported platform: $OSTYPE"
    exit 1
fi

echo "Building backend for $PLATFORM..."
dotnet publish -c $CONFIG -r $RID -o $BACKEND_OUTPUT_DIR --self-contained true /p:PublishSingleFile=true

echo "✓ Backend build successful"
echo ""

# Step 2: Copy backend to Tauri sidecar location
echo "Step 2: Preparing Tauri sidecar binaries..."
echo "---------------------------------------------"

TAURI_SIDECAR_DIR="tauri/src-tauri/binaries"

# Create sidecar directory if it doesn't exist
mkdir -p "$TAURI_SIDECAR_DIR"

# Copy backend executable with platform-specific naming
BACKEND_EXE="$BACKEND_OUTPUT_DIR/smart-compressor"
SIDECAR_EXE="$TAURI_SIDECAR_DIR/smart-compressor-backend-$SIDECAR_SUFFIX"

if [ -f "$BACKEND_EXE" ]; then
    cp "$BACKEND_EXE" "$SIDECAR_EXE"
    chmod +x "$SIDECAR_EXE"
    echo "✓ Copied backend to: $SIDECAR_EXE"
else
    echo "✗ Backend executable not found at: $BACKEND_EXE"
    exit 1
fi

echo ""

# Step 3: Build Tauri application
echo "Step 3: Building Tauri application..."
echo "---------------------------------------------"

cd tauri

# Install npm dependencies if needed
if [ ! -d "node_modules" ]; then
    echo "Installing npm dependencies..."
    npm install
fi

# Build Tauri app
if [ "$RELEASE" = true ]; then
    echo "Building Tauri app (Release)..."
    npm run tauri build
else
    echo "Building Tauri app (Debug)..."
    npm run tauri build -- --debug
fi

cd ..

echo "✓ Tauri build successful"
echo ""

# Step 4: Copy final executable to publish-win
echo "Step 4: Copying final executable to publish-win..."
echo "---------------------------------------------"

PUBLISH_DIR="publish-win"
mkdir -p "$PUBLISH_DIR"

if [ "$RELEASE" = true ]; then
    BUNDLE_DIR="tauri/src-tauri/target/release/bundle"
    
    # Try to find and copy the built executable
    if [ -f "$BUNDLE_DIR/macos/Smart Video Compressor.app/Contents/MacOS/smart-compressor" ]; then
        cp "$BUNDLE_DIR/macos/Smart Video Compressor.app/Contents/MacOS/smart-compressor" "$PUBLISH_DIR/smart-compressor"
        echo "✓ Copied macOS app to: $PUBLISH_DIR/smart-compressor"
    elif [ -f "$BUNDLE_DIR/appimage/smart-compressor.AppImage" ]; then
        cp "$BUNDLE_DIR/appimage/smart-compressor.AppImage" "$PUBLISH_DIR/smart-compressor"
        echo "✓ Copied AppImage to: $PUBLISH_DIR/smart-compressor"
    else
        BACKEND_BIN="tauri/src-tauri/target/release/smart-compressor-backend"
        if [ -f "$BACKEND_BIN" ]; then
            cp "$BACKEND_BIN" "$PUBLISH_DIR/smart-compressor"
            chmod +x "$PUBLISH_DIR/smart-compressor"
            echo "✓ Copied backend to: $PUBLISH_DIR/smart-compressor"
        fi
    fi
else
    DEBUG_BIN="tauri/src-tauri/target/debug/smart-compressor"
    if [ -f "$DEBUG_BIN" ]; then
        cp "$DEBUG_BIN" "$PUBLISH_DIR/smart-compressor-debug"
        chmod +x "$PUBLISH_DIR/smart-compressor-debug"
        echo "✓ Copied debug executable to: $PUBLISH_DIR/smart-compressor-debug"
    fi
fi

echo ""

# Step 5: Show output location
echo "================================================"
echo "Build Complete!"
echo "================================================"
echo ""

if [ -f "$PUBLISH_DIR/smart-compressor" ]; then
    FILE_SIZE=$(du -h "$PUBLISH_DIR/smart-compressor" | cut -f1)
    echo "Final executable: $PUBLISH_DIR/smart-compressor"
    echo "File size: $FILE_SIZE"
else
    if [ "$RELEASE" = true ]; then
        BUNDLE_DIR="tauri/src-tauri/target/release/bundle"
        echo "Build artifacts: $BUNDLE_DIR"
        echo ""
        
        if [ -d "$BUNDLE_DIR" ]; then
            echo "Available bundles:"
            ls -1 "$BUNDLE_DIR"
        fi
    else
        echo "Debug build output: tauri/src-tauri/target/debug/"
    fi
fi

echo ""
echo "To run the app:"
if [ "$RELEASE" = true ] && [ -f "$PUBLISH_DIR/smart-compressor" ]; then
    echo "  ./$PUBLISH_DIR/smart-compressor"
else
    echo "  cd tauri"
    echo "  npm run tauri dev"
fi
echo ""

