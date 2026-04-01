//! Link Windows graphics libraries for DXGI / D3D11 capture.
//! Also copy `FFmpeg` `bin\\*.dll` next to test artifacts (`cargo test -p liteclip-core`) so
//! `ffmpeg-next` can load (see repo root `build.rs` for the main app).

use std::fs;
use std::path::PathBuf;

fn cargo_target_profile_dir() -> Option<PathBuf> {
    let profile = std::env::var("PROFILE").ok()?;
    if let Ok(target) = std::env::var("CARGO_TARGET_DIR") {
        return Some(PathBuf::from(target).join(&profile));
    }
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").ok()?);
    out_dir.ancestors().nth(3).map(PathBuf::from)
}

#[cfg(windows)]
fn resolve_ffmpeg_bin_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("FFMPEG_DIR") {
        return PathBuf::from(dir).join("bin");
    }
    let manifest =
        PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string()));
    manifest.join("../../ffmpeg_dev/sdk/bin")
}

#[cfg(windows)]
fn copy_ffmpeg_runtime_dlls() {
    println!("cargo:rerun-if-env-changed=FFMPEG_DIR");

    let Some(profile_dir) = cargo_target_profile_dir() else {
        return;
    };

    let ffmpeg_bin_dir = resolve_ffmpeg_bin_dir();
    if !ffmpeg_bin_dir.is_dir() {
        return;
    }

    println!("cargo:rerun-if-changed={}", ffmpeg_bin_dir.display());

    let deps_dir = profile_dir.join("deps");
    let _ = fs::create_dir_all(&profile_dir);
    let _ = fs::create_dir_all(&deps_dir);

    let Ok(entries) = fs::read_dir(&ffmpeg_bin_dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path
            .extension()
            .and_then(|s| s.to_str())
            .map(|e| !e.eq_ignore_ascii_case("dll"))
            .unwrap_or(true)
        {
            continue;
        }
        let Some(name) = path.file_name().map(PathBuf::from) else {
            continue;
        };
        for dest_dir in [&profile_dir, &deps_dir] {
            let _ = fs::copy(&path, dest_dir.join(&name));
        }
    }
}

fn main() {
    #[cfg(windows)]
    {
        copy_ffmpeg_runtime_dlls();

        println!("cargo:rustc-link-lib=d3d11");
        println!("cargo:rustc-link-lib=dxgi");
        println!("cargo:rustc-link-lib=dxguid");
        println!("cargo:rustc-link-lib=user32");
        println!("cargo:rustc-link-lib=gdi32");
    }
}
