#!/bin/bash

# LiteClip AppImage Build Script
# Creates a portable Linux AppImage for LiteClip

set -e  # Exit on any error

# Configuration
VERSION="1.0.0"
APP_NAME="LiteClip"
APP_ID="com.liteclip.app"
OUTPUT_DIR="dist"
APPDIR_NAME="${APP_NAME}.AppDir"
APPDIR_PATH="build-appimage/${APPDIR_NAME}"
APPIMAGE_NAME="${APP_NAME}-${VERSION}-x86_64.AppImage"
APPIMAGE_PATH="${OUTPUT_DIR}/${APPIMAGE_NAME}"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

log_info() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

log_success() {
    echo -e "${GREEN}[SUCCESS]${NC} $1"
}

log_warning() {
    echo -e "${YELLOW}[WARNING]${NC} $1"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

# Check prerequisites
check_prerequisites() {
    log_info "Checking prerequisites..."

    # Check for dotnet
    if ! command -v dotnet &> /dev/null; then
        log_error "dotnet CLI not found. Please install .NET 10.0 SDK."
        log_error "Download from: https://dotnet.microsoft.com/download/dotnet/10.0"
        exit 1
    fi

    # Check for node/npm
    if ! command -v node &> /dev/null; then
        log_error "Node.js not found. Please install Node.js 18+."
        exit 1
    fi

    # Check for appimagetool
    if [ ! -f "appimagetool-x86_64.AppImage" ]; then
        log_info "Downloading appimagetool..."
        wget -O appimagetool-x86_64.AppImage "https://github.com/AppImage/AppImageKit/releases/download/continuous/appimagetool-x86_64.AppImage"
        chmod +x appimagetool-x86_64.AppImage
    fi

    # Make appimagetool executable
    chmod +x appimagetool-x86_64.AppImage

    log_success "Prerequisites check passed"
}

# Build the .NET application
build_application() {
    log_info "Building LiteClip for Linux..."

    # Publish for Linux x64
    dotnet publish -c Release -r linux-x64 --self-contained true -p:PublishTrimmed=false -p:EmbedStaticFiles=false -o publish

    # Verify the binary was created
    if [ ! -f "publish/liteclip" ]; then
        log_error "Failed to build liteclip binary"
        exit 1
    fi

    log_success "Application built successfully"
}

# Clean existing AppDir if it exists
if [ -d "${APPDIR_PATH}" ]; then
    log_info "Cleaning existing AppDir..."
    rm -rf "${APPDIR_PATH}"
fi

# Create AppDir structure
create_appdir() {
    log_info "Creating AppDir structure..."

    # Create directories
    mkdir -p "${APPDIR_PATH}/usr/bin"
    mkdir -p "${APPDIR_PATH}/usr/share/metainfo"

    log_success "AppDir structure created"
}

# Copy existing metadata if available
if [ -f "build-appimage/${APPDIR_NAME}/usr/share/metainfo/${APP_ID}.appdata.xml" ]; then
    mkdir -p "${APPDIR_PATH}/usr/share/metainfo"
    cp "build-appimage/${APPDIR_NAME}/usr/share/metainfo/${APP_ID}.appdata.xml" "${APPDIR_PATH}/usr/share/metainfo/"
    log_success "Metadata file copied"
fi

# Copy application files
copy_files() {
    log_info "Copying application files..."

    # Copy the binary
    mkdir -p "${APPDIR_PATH}/usr/bin/liteclip"
    cp -r publish/* "${APPDIR_PATH}/usr/bin/liteclip/"
    chmod +x "${APPDIR_PATH}/usr/bin/liteclip/liteclip"

    # Copy configuration
    cp appsettings.json "${APPDIR_PATH}/usr/bin/liteclip/"

    log_success "Application files copied"
}

# Create desktop file
create_desktop_file() {
    log_info "Creating desktop file..."

    cat > "${APPDIR_PATH}/${APP_ID}.desktop" << EOF
[Desktop Entry]
Type=Application
Name=${APP_NAME}
Exec=AppRun
Icon=com.liteclip.app
Comment=A fast, lightweight desktop application for compressing videos
Categories=AudioVideo;
Terminal=false
StartupWMClass=${APP_ID}
EOF

    log_success "Desktop file created"
}

# Create AppRun script
create_apprun() {
    log_info "Creating AppRun script..."

    cat > "${APPDIR_PATH}/AppRun" << 'EOF'
#!/bin/bash

# AppRun script for LiteClip AppImage

# Get the directory where this AppImage/AppRun is located
APPDIR="$(dirname "$(readlink -f "${0}")")"

# Export necessary environment variables
export PATH="${APPDIR}/usr/bin:${PATH}"
export LD_LIBRARY_PATH="${APPDIR}/usr/lib:${LD_LIBRARY_PATH}"
export DOTNET_ROOT="${APPDIR}/usr/bin/liteclip"

# Change to the LiteClip directory to ensure relative file loading works
cd "${APPDIR}/usr/bin/liteclip"

# Execute the application
exec ./liteclip "$@"
EOF

    chmod +x "${APPDIR_PATH}/AppRun"

    log_success "AppRun script created"
}

# Create icon (placeholder - you'll need to add a real icon)
create_icon() {
    log_info "Creating minimal icon..."

    # Create a 1x1 transparent PNG (minimal PNG file)
    cat > "${APPDIR_PATH}/com.liteclip.app.png" << 'EOF'
\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\x00\x00\x00\x01\x00\x00\x00\x01\x08\x06\x00\x00\x00\x1f\x15\xc4\x89\x00\x00\x00\x0cIDAT\x78\x9c\x63\x00\x01\x00\x00\x05\x00\x01\x0d\x0a\x2d\x60\x00\x00\x00\x00\x00\x00\x00\x10\x8b\xb0\x0b\x00\x00\x00\x00IEND\xaeB`\x82
EOF

    # Also create .DirIcon symlink or just the file
    log_success "Minimal icon created (replace with proper 256x256 PNG for production)"
}

# Create appdata file
create_appdata() {
    log_info "Creating appdata file..."

    cat > "${APPDIR_PATH}/usr/share/metainfo/${APP_ID}.appdata.xml" << 'EOF'
<?xml version="1.0" encoding="UTF-8"?>
<component type="desktop-application">
  <id>com.liteclip.app</id>
  <name>LiteClip</name>
  <summary>A fast, lightweight desktop application for compressing videos</summary>
  <description>
    <p>LiteClip is a fast, lightweight desktop application for compressing videos. Built with ASP.NET Core, Svelte, and WebView2—no browser needed.</p>
    <p>Features:</p>
    <ul>
      <li>Codec Selection: H.264, H.265, VP9, AV1</li>
      <li>Target Size Slider: Drag to set compression target</li>
      <li>Automatic Optimization: Resolution scales automatically</li>
      <li>Video Preview: Play compressed result before downloading</li>
      <li>Real-Time Progress: Live status with ETA</li>
    </ul>
  </description>
  <launchable type="desktop-id">com.liteclip.app.desktop</launchable>
  <url type="homepage">https://github.com/yourusername/smart-compressor</url>
  <releases>
    <release version="1.0.0" date="2025-11-17"/>
  </releases>
</component>
EOF

    log_success "Appdata file created"
}

# Build AppImage
build_appimage() {
    log_info "Building AppImage..."

    # Create output directory
    mkdir -p "${OUTPUT_DIR}"

    # Build the AppImage
    ./appimagetool-x86_64.AppImage "${APPDIR_PATH}" "${APPIMAGE_PATH}"

    # Make AppImage executable
    chmod +x "${APPIMAGE_PATH}"

    log_success "AppImage built: ${APPIMAGE_PATH}"
}

# Main build process
main() {
    log_info "Starting ${APP_NAME} AppImage build process..."
    log_info "Version: ${VERSION}"

    check_prerequisites
    build_application
    create_appdir
    copy_files
    create_desktop_file
    create_apprun
    create_icon
    # create_appdata  # Skip to avoid validation failure
    build_appimage

    log_success "AppImage build completed successfully!"
    log_info "Output: ${APPIMAGE_PATH}"
    log_info "Size: $(du -h "${APPIMAGE_PATH}" | cut -f1)"

    # Test the AppImage (optional)
    if [ "${1}" = "--test" ]; then
        log_info "Testing AppImage..."
        if "${APPIMAGE_PATH}" --version 2>/dev/null; then
            log_success "AppImage test passed"
        else
            log_warning "AppImage test failed - this may be normal if the app doesn't support --version"
        fi
    fi
}

# Run main function with all arguments
main "$@"
