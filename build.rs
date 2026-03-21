use std::fs;
use std::path::PathBuf;

/// Where `liteclip-replay.exe` / `liteclip_core.dll` tests land: `target/debug` or `target/release`.
fn cargo_target_profile_dir() -> Option<PathBuf> {
    let profile = std::env::var("PROFILE").ok()?;
    if let Ok(target) = std::env::var("CARGO_TARGET_DIR") {
        return Some(PathBuf::from(target).join(&profile));
    }
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").ok()?);
    // OUT_DIR ≈ target/debug/build/<crate-hash>/out → ancestors: out, hash, build, debug
    out_dir.ancestors().nth(3).map(PathBuf::from)
}

/// `ffmpeg-next` links FFmpeg DLLs dynamically on Windows. The loader must find them next to the
/// `.exe` or on `PATH`, otherwise the process exits with **STATUS_DLL_NOT_FOUND (0xc0000135)**
/// before `main`.
///
/// Copies every `*.dll` from the FFmpeg SDK `bin` directory into `target/<profile>/`.
#[cfg(windows)]
fn copy_runtime_dlls() {
    println!("cargo:rerun-if-env-changed=FFMPEG_DIR");

    let Some(profile_dir) = cargo_target_profile_dir() else {
        println!(
            "cargo:warning=liteclip: unable to locate target profile dir (OUT_DIR/CARGO_TARGET_DIR); skipping FFmpeg DLL copy"
        );
        return;
    };

    let manifest_dir =
        PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string()));
    let ffmpeg_bin_dir = if let Ok(dir) = std::env::var("FFMPEG_DIR") {
        PathBuf::from(dir).join("bin")
    } else {
        manifest_dir.join("ffmpeg_dev").join("sdk").join("bin")
    };

    if !ffmpeg_bin_dir.is_dir() {
        println!(
            "cargo:warning=liteclip: FFmpeg SDK bin directory not found at {}. \
Set FFMPEG_DIR to your FFmpeg root (with bin\\ containing avcodec-*.dll, etc.) or place the SDK at \
ffmpeg_dev/sdk/bin under the repo. Without those DLLs beside the executable, cargo run fails with \
STATUS_DLL_NOT_FOUND (0xc0000135).",
            ffmpeg_bin_dir.display()
        );
        return;
    }

    println!("cargo:rerun-if-changed={}", ffmpeg_bin_dir.display());

    let deps_dir = profile_dir.join("deps");
    let _ = fs::create_dir_all(&profile_dir);
    let _ = fs::create_dir_all(&deps_dir);

    let entries = match fs::read_dir(&ffmpeg_bin_dir) {
        Ok(e) => e,
        Err(e) => {
            println!(
                "cargo:warning=liteclip: cannot read FFmpeg bin dir {}: {}",
                ffmpeg_bin_dir.display(),
                e
            );
            return;
        }
    };

    let mut copied = 0usize;
    let mut failed = 0usize;

    for entry in entries.flatten() {
        let path = entry.path();
        if path
            .extension()
            .and_then(|s| s.to_str())
            .map(|e| e.eq_ignore_ascii_case("dll"))
            != Some(true)
        {
            continue;
        }
        let Some(name) = path.file_name().map(PathBuf::from) else {
            continue;
        };
        // Main binary (e.g. liteclip-replay.exe) loads DLLs from target/debug/.
        for dest_dir in [&profile_dir, &deps_dir] {
            let destination = dest_dir.join(&name);
            match fs::copy(&path, &destination) {
                Ok(_) => copied += 1,
                Err(err) => {
                    failed += 1;
                    println!(
                        "cargo:warning=liteclip: failed to copy {} to {}: {}",
                        path.display(),
                        destination.display(),
                        err
                    );
                }
            }
        }
    }

    if copied == 0 {
        println!(
            "cargo:warning=liteclip: no .dll files were copied from {}. \
Add FFmpeg shared libraries there (or set FFMPEG_DIR) to fix 0xc0000135 at startup.",
            ffmpeg_bin_dir.display()
        );
    } else if failed > 0 {
        println!(
            "cargo:warning=liteclip: {} FFmpeg DLL copy operation(s) failed (see warnings above).",
            failed
        );
    }
}

#[cfg(not(windows))]
fn copy_runtime_dlls() {}

fn generate_windows_icon(source: &str, output: &PathBuf) {
    let image = image::open(source).expect("failed to open logo.ico for windows icon generation");
    let rgba = image.into_rgba8();
    let mut icon_dir = ico::IconDir::new(ico::ResourceType::Icon);

    for size in [16u32, 32, 48, 64, 128, 256] {
        let resized =
            image::imageops::resize(&rgba, size, size, image::imageops::FilterType::Lanczos3);
        let icon_image = ico::IconImage::from_rgba_data(size, size, resized.into_raw());
        let icon_entry =
            ico::IconDirEntry::encode(&icon_image).expect("failed to encode icon entry");
        icon_dir.add_entry(icon_entry);
    }

    let icon_file = fs::File::create(output).expect("failed to create generated windows icon");
    icon_dir
        .write(icon_file)
        .expect("failed to write generated windows icon");
}

fn main() {
    copy_runtime_dlls();

    // Windows-specific linking
    #[cfg(windows)]
    {
        let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
        let generated_icon = out_dir.join("logo.windows.ico");
        generate_windows_icon("logo.ico", &generated_icon);

        // Link required Windows libraries for D3D11
        println!("cargo:rustc-link-lib=d3d11");
        println!("cargo:rustc-link-lib=dxgi");
        println!("cargo:rustc-link-lib=dxguid");

        // User32 for window/message functions
        println!("cargo:rustc-link-lib=user32");

        // GDI32 for graphics functions
        println!("cargo:rustc-link-lib=gdi32");

        let mut res = winres::WindowsResource::new();
        res.set_icon(
            generated_icon
                .to_str()
                .expect("generated icon path must be valid utf-8"),
        );
        res.compile().unwrap();
    }

    println!("cargo:rerun-if-changed=logo.ico");
    println!("cargo:rerun-if-changed=build.rs");
}
