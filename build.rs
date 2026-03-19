use std::fs;
use std::path::PathBuf;
fn cargo_target_profile_dir() -> Option<PathBuf> {
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").ok()?);
    out_dir.ancestors().nth(3).map(PathBuf::from)
}

fn copy_runtime_dlls() {
    let Some(profile_dir) = cargo_target_profile_dir() else {
        println!(
            "cargo:warning=Unable to locate Cargo profile output directory for runtime DLL copy"
        );
        return;
    };

    let ffmpeg_bin_dir = PathBuf::from("ffmpeg_dev/sdk/bin");
    let required_dlls = [
        "avcodec-61.dll",
        "avformat-61.dll",
        "avutil-59.dll",
        "swresample-5.dll",
        "swscale-8.dll",
    ];

    let _ = fs::create_dir_all(&profile_dir);

    for dll_name in required_dlls {
        let source = ffmpeg_bin_dir.join(dll_name);
        println!("cargo:rerun-if-changed={}", source.display());

        if !source.exists() {
            println!(
                "cargo:warning=Required FFmpeg runtime DLL not found: {}",
                source.display()
            );
            continue;
        }

        let destination = profile_dir.join(dll_name);
        match fs::copy(&source, &destination) {
            Ok(_) => {}
            Err(err) => println!(
                "cargo:warning=Failed to copy {} to {}: {}",
                source.display(),
                destination.display(),
                err
            ),
        }
    }
}

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
