use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

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

fn find_fxc_exe() -> Option<PathBuf> {
    // Try to find fxc.exe in Windows SDK
    let kits_root = PathBuf::from("C:/Program Files (x86)/Windows Kits/10/bin");
    if let Ok(entries) = fs::read_dir(&kits_root) {
        for entry in entries.flatten() {
            let version_dir = entry.path();
            let fxc_path = version_dir.join("x64/fxc.exe");
            if fxc_path.exists() {
                return Some(fxc_path);
            }
        }
    }
    None
}

fn compile_shader(
    fxc_path: &Path,
    shader_type: &str,
    entry_point: &str,
    input: &str,
    output: &str,
) -> bool {
    // Use PowerShell to run fxc.exe (handles paths with spaces)
    let ps_cmd = format!(
        "& '{}' /T {} /E {} /Fo '{}' '{}'",
        fxc_path.display(),
        shader_type,
        entry_point,
        output,
        input
    );

    Command::new("powershell.exe")
        .args(["-Command", &ps_cmd])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
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

    // Compile shaders
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let vs_output = PathBuf::from(&out_dir).join("vs_simple.cso");
    let ps_output = PathBuf::from(&out_dir).join("ps_simple.cso");

    // Find fxc.exe in Windows SDK
    let fxc_path = match find_fxc_exe() {
        Some(path) => path,
        None => {
            println!("cargo:warning=fxc.exe not found in Windows SDK, GPU scaling disabled");
            fs::write(&vs_output, []).ok();
            fs::write(&ps_output, []).ok();
            println!("cargo:rerun-if-changed=shaders/vs_simple.hlsl");
            println!("cargo:rerun-if-changed=shaders/ps_simple.hlsl");
            println!("cargo:rerun-if-changed=build.rs");
            return;
        }
    };

    // Compile vertex shader
    if compile_shader(
        &fxc_path,
        "vs_5_0",
        "main",
        "shaders/vs_simple.hlsl",
        vs_output.to_str().unwrap(),
    ) {
    } else {
        fs::write(&vs_output, []).ok();
        println!("cargo:warning=Vertex shader compilation failed, GPU scaling disabled");
    }

    // Compile pixel shader
    if compile_shader(
        &fxc_path,
        "ps_5_0",
        "main",
        "shaders/ps_simple.hlsl",
        ps_output.to_str().unwrap(),
    ) {
    } else {
        fs::write(&ps_output, []).ok();
        println!("cargo:warning=Pixel shader compilation failed, GPU scaling disabled");
    }

    // Tell Cargo to re-run if shaders change
    println!("cargo:rerun-if-changed=shaders/vs_simple.hlsl");
    println!("cargo:rerun-if-changed=shaders/ps_simple.hlsl");
    println!("cargo:rerun-if-changed=logo.ico");

    // Re-run if build.rs changes
    println!("cargo:rerun-if-changed=build.rs");
}
