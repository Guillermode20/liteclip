#!/bin/bash

# Script to build LiteClip as an RPM package for Fedora
# Version: 1.0.0

set -e  # Exit on any error

VERSION="1.0.0"
RELEASE="1"
APP_NAME="liteclip"
DIST_DIR="dist"
RPMBUILD_DIR="$HOME/rpmbuild"

echo "=== LiteClip - RPM Build Script ==="
echo ""
echo "Version: $VERSION"
echo "Release: $RELEASE"
echo ""

# Check if rpmbuild is installed
if ! command -v rpmbuild &> /dev/null; then
    echo "ERROR: rpmbuild not found."
    echo "Install it with: sudo dnf install rpm-build rpmdevtools"
    exit 1
fi

# First, build the Linux binary
echo "Step 1: Building Linux binary..."
bash build-linux.sh

if [ $? -ne 0 ]; then
    echo "ERROR: Linux build failed"
    exit 1
fi

# Setup RPM build directory structure
echo ""
echo "Step 2: Setting up RPM build directories..."
mkdir -p "$RPMBUILD_DIR"/{BUILD,RPMS,SOURCES,SPECS,SRPMS}

# Create source tarball
echo "Creating source tarball..."
TARBALL_DIR="${APP_NAME}-${VERSION}"
TARBALL_NAME="${APP_NAME}-${VERSION}.tar.gz"

# Create temporary directory for tarball
rm -rf "$TARBALL_DIR"
mkdir -p "$TARBALL_DIR"

# Copy the binary
cp publish-linux/liteclip "$TARBALL_DIR/"

# Create tarball
tar -czf "$TARBALL_NAME" "$TARBALL_DIR"

# Move tarball to SOURCES
mv "$TARBALL_NAME" "$RPMBUILD_DIR/SOURCES/"

# Clean up temporary directory
rm -rf "$TARBALL_DIR"

echo "Source tarball created"

# Copy spec file to SPECS directory
echo "Copying spec file..."
cp liteclip.spec "$RPMBUILD_DIR/SPECS/"

# Build the RPM
echo ""
echo "Step 3: Building RPM package..."
cd "$RPMBUILD_DIR/SPECS"
rpmbuild -ba liteclip.spec

if [ $? -ne 0 ]; then
    echo "ERROR: RPM build failed"
    exit 1
fi

# Find the generated RPM
echo ""
echo "Step 4: Locating generated RPM..."
RPM_FILE=$(find "$RPMBUILD_DIR/RPMS" -name "${APP_NAME}-${VERSION}-${RELEASE}.*.rpm" -type f | head -n 1)

if [ -z "$RPM_FILE" ]; then
    echo "ERROR: Could not find generated RPM file"
    exit 1
fi

echo "Found RPM: $RPM_FILE"

# Move to dist directory
echo ""
echo "Step 5: Moving to dist directory..."
cd - > /dev/null
mkdir -p "$DIST_DIR"
cp "$RPM_FILE" "$DIST_DIR/"

RPM_FILENAME=$(basename "$RPM_FILE")

# Display summary
echo ""
echo "=== RPM Build Complete ==="
echo ""
echo "Output location: $DIST_DIR/$RPM_FILENAME"
echo ""
echo "✨ RPM package created successfully!"
echo ""
echo "To install:"
echo "  sudo dnf install $DIST_DIR/$RPM_FILENAME"
echo ""
echo "Or:"
echo "  sudo rpm -ivh $DIST_DIR/$RPM_FILENAME"
echo ""
echo "Requirements (will be checked during install):"
echo "  - webkit2gtk4.0"
echo "  - FFmpeg (install separately: sudo dnf install ffmpeg)"
echo ""
echo "After installation, run with:"
echo "  liteclip"
echo ""

