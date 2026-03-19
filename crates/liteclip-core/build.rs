use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn find_fxc_exe() -> Option<PathBuf> {
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
    #[cfg(windows)]
    {
        println!("cargo:rustc-link-lib=d3d11");
        println!("cargo:rustc-link-lib=dxgi");
        println!("cargo:rustc-link-lib=dxguid");
        println!("cargo:rustc-link-lib=user32");
        println!("cargo:rustc-link-lib=gdi32");
    }

    let out_dir = std::env::var("OUT_DIR").unwrap();
    let vs_output = PathBuf::from(&out_dir).join("vs_simple.cso");
    let ps_output = PathBuf::from(&out_dir).join("ps_simple.cso");

    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let workspace_root = manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .expect("liteclip-core must live at crates/liteclip-core under workspace root");
    let vs_hlsl = workspace_root.join("shaders/vs_simple.hlsl");
    let ps_hlsl = workspace_root.join("shaders/ps_simple.hlsl");

    let fxc_path = match find_fxc_exe() {
        Some(path) => path,
        None => {
            println!("cargo:warning=fxc.exe not found in Windows SDK, GPU scaling disabled");
            fs::write(&vs_output, []).ok();
            fs::write(&ps_output, []).ok();
            println!("cargo:rerun-if-changed={}", vs_hlsl.display());
            println!("cargo:rerun-if-changed={}", ps_hlsl.display());
            println!("cargo:rerun-if-changed=build.rs");
            return;
        }
    };

    if compile_shader(
        &fxc_path,
        "vs_5_0",
        "main",
        vs_hlsl.to_str().unwrap(),
        vs_output.to_str().unwrap(),
    ) {
    } else {
        fs::write(&vs_output, []).ok();
        println!("cargo:warning=Vertex shader compilation failed, GPU scaling disabled");
    }

    if compile_shader(
        &fxc_path,
        "ps_5_0",
        "main",
        ps_hlsl.to_str().unwrap(),
        ps_output.to_str().unwrap(),
    ) {
    } else {
        fs::write(&ps_output, []).ok();
        println!("cargo:warning=Pixel shader compilation failed, GPU scaling disabled");
    }

    println!("cargo:rerun-if-changed={}", vs_hlsl.display());
    println!("cargo:rerun-if-changed={}", ps_hlsl.display());
    println!("cargo:rerun-if-changed=build.rs");
}
