Name:           liteclip
Version:        1.0.0
Release:        1%{?dist}
Summary:        Fast, lightweight desktop application for compressing videos

License:        Proprietary
URL:            https://github.com/yourusername/liteclip
Source0:        %{name}-%{version}.tar.gz

# Dependencies
Requires:       webkit2gtk4.0

# Since we're using a self-contained build, we don't strictly need dotnet-runtime
# But if users have it installed, it won't hurt
# Requires:       dotnet-runtime-9.0

# Build requirements
BuildRequires:  tar

%description
LiteClip is a fast, lightweight desktop application for compressing videos.
Built with ASP.NET Core, Svelte, and WebView2—no browser needed.

Features:
- Codec Selection: H.264, H.265, VP9, AV1
- Target Size Slider: Drag to set compression target (1-100%% of original)
- Automatic Optimization: Resolution scales automatically to hit target size
- Video Preview: Play compressed result before downloading
- Native Desktop Window: WebView2-based UI, no browser required
- Single Executable: Self-contained app

Note: FFmpeg must be installed separately for video compression functionality.

%prep
%setup -q

%build
# No build needed - binary is pre-compiled

%install
rm -rf $RPM_BUILD_ROOT
mkdir -p $RPM_BUILD_ROOT%{_bindir}
install -m 0755 liteclip $RPM_BUILD_ROOT%{_bindir}/liteclip

%files
%{_bindir}/liteclip

%post
echo ""
echo "==================================================================="
echo "LiteClip has been installed successfully!"
echo "==================================================================="
echo ""
echo "IMPORTANT: FFmpeg is required for video compression."
echo ""
echo "Install FFmpeg with:"
echo "  sudo dnf install ffmpeg"
echo ""
echo "To run LiteClip:"
echo "  liteclip"
echo ""
echo "==================================================================="
echo ""

%postun
# Clean up temp directory if package is removed
rm -rf /tmp/liteclip-ffmpeg 2>/dev/null || true

%changelog
* Thu Nov 13 2025 LiteClip Team <team@liteclip.example> - 1.0.0-1
- Initial RPM package release
- Self-contained .NET 9.0 application
- Supports H.264, H.265, VP9, and AV1 codecs
- Minimal desktop integration (command-line only)

