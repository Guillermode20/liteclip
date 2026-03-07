use std::fs;
use std::path::PathBuf;
use std::process::Command;

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
    fxc_path: &PathBuf,
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
    // Windows-specific linking
    #[cfg(windows)]
    {
        // Link required Windows libraries for D3D11
        println!("cargo:rustc-link-lib=d3d11");
        println!("cargo:rustc-link-lib=dxgi");
        println!("cargo:rustc-link-lib=dxguid");

        // User32 for window/message functions
        println!("cargo:rustc-link-lib=user32");

        // GDI32 for graphics functions
        println!("cargo:rustc-link-lib=gdi32");

        let mut res = winres::WindowsResource::new();
        res.set_icon("app.ico");
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

    // Re-run if build.rs changes
    println!("cargo:rerun-if-changed=build.rs");
}
