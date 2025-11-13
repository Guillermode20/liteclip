#!/bin/bash

# Script to build LiteClip as an AppImage
# Version: 1.0.0

set -e  # Exit on any error

VERSION="1.0.0"
APP_NAME="LiteClip"
APPDIR="AppDir"
OUTPUT_NAME="${APP_NAME}-${VERSION}-x86_64.AppImage"
DIST_DIR="dist"

echo "=== LiteClip - AppImage Build Script ==="
echo ""
echo "Version: $VERSION"
echo "Output: $OUTPUT_NAME"
echo ""

# First, build the Linux binary
echo "Step 1: Building Linux binary..."
bash build-linux.sh

if [ $? -ne 0 ]; then
    echo "ERROR: Linux build failed"
    exit 1
fi

# Clean previous AppImage build
echo ""
echo "Step 2: Preparing AppImage directory..."
if [ -d "$APPDIR" ]; then
    rm -rf "$APPDIR"
fi

# Create AppImage directory structure
mkdir -p "$APPDIR/usr/bin"
mkdir -p "$APPDIR/usr/share/applications"
mkdir -p "$APPDIR/usr/share/metainfo"

# Copy the binary
echo "Copying binary..."
cp publish-linux/liteclip "$APPDIR/usr/bin/"
chmod +x "$APPDIR/usr/bin/liteclip"

# Create desktop file (minimal, with placeholder icon reference)
echo "Creating desktop file..."
cat > "$APPDIR/liteclip.desktop" << 'EOF'
[Desktop Entry]
Type=Application
Name=LiteClip
Comment=Fast Video Compression
Exec=liteclip
Icon=liteclip
Categories=AudioVideo;Video;
Terminal=false
EOF

# Create a minimal 256x256 PNG icon using ImageMagick (if available)
# Otherwise create an empty placeholder
echo "Creating placeholder icon..."
if command -v convert &> /dev/null; then
    # Create a simple icon with ImageMagick
    convert -size 256x256 xc:transparent -fill '#4A90E2' -draw "circle 128,128 128,28" "$APPDIR/liteclip.png"
    echo "Created icon with ImageMagick"
else
    # Create a minimal valid PNG file (1x1 transparent pixel)
    # PNG header + IHDR + IDAT + IEND chunks
    echo "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==" | base64 -d > "$APPDIR/liteclip.png"
    echo "Created minimal placeholder icon"
fi

# Create AppRun script
echo "Creating AppRun launcher..."
cat > "$APPDIR/AppRun" << 'EOF'
#!/bin/bash
SELF=$(readlink -f "$0")
HERE=${SELF%/*}
export PATH="${HERE}/usr/bin:${PATH}"
export LD_LIBRARY_PATH="${HERE}/usr/lib:${LD_LIBRARY_PATH}"
exec "${HERE}/usr/bin/liteclip" "$@"
EOF

chmod +x "$APPDIR/AppRun"

# Download appimagetool if not present
echo ""
echo "Step 3: Preparing appimagetool..."
APPIMAGETOOL="appimagetool-x86_64.AppImage"

if [ ! -f "$APPIMAGETOOL" ]; then
    echo "Downloading appimagetool..."
    wget -q "https://github.com/AppImage/AppImageKit/releases/download/continuous/$APPIMAGETOOL"
    chmod +x "$APPIMAGETOOL"
    echo "appimagetool downloaded"
else
    echo "Using existing appimagetool"
fi

# Build the AppImage
echo ""
echo "Step 4: Building AppImage..."
ARCH=x86_64 ./"$APPIMAGETOOL" --no-appstream "$APPDIR" "$OUTPUT_NAME"

if [ $? -ne 0 ]; then
    echo "ERROR: AppImage build failed"
    exit 1
fi

# Move to dist directory
echo ""
echo "Step 5: Moving to dist directory..."
mkdir -p "$DIST_DIR"
mv "$OUTPUT_NAME" "$DIST_DIR/"

# Clean up
echo "Cleaning up temporary files..."
rm -rf "$APPDIR"

# Display summary
echo ""
echo "=== AppImage Build Complete ==="
echo ""
echo "Output location: $DIST_DIR/$OUTPUT_NAME"
echo ""
echo "✨ Portable AppImage created successfully!"
echo "   This is a completely self-contained executable (except FFmpeg)."
echo "   You can run it on any Linux distribution."
echo ""
echo "Requirements:"
echo "  - webkit2gtk4.0 (usually pre-installed)"
echo "  - FFmpeg (install separately: sudo dnf install ffmpeg)"
echo ""
echo "To run:"
echo "  chmod +x $DIST_DIR/$OUTPUT_NAME"
echo "  ./$DIST_DIR/$OUTPUT_NAME"
echo ""

