use std::collections::HashMap;
use std::fs;
use std::hash::{DefaultHasher, Hasher};
use std::io::Read;
use std::path::PathBuf;

/// Where `liteclip.exe` / `liteclip_core.dll` tests land: `target/debug` or `target/release`.
fn cargo_target_profile_dir() -> Option<PathBuf> {
    let profile = std::env::var("PROFILE").ok()?;
    if let Ok(target) = std::env::var("CARGO_TARGET_DIR") {
        return Some(PathBuf::from(target).join(&profile));
    }
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").ok()?);
    // OUT_DIR ≈ target/debug/build/<crate-hash>/out → ancestors: out, hash, build, debug
    out_dir.ancestors().nth(3).map(PathBuf::from)
}

/// Compute a deterministic hash of a file's contents using DefaultHasher.
/// Used for change detection: if the hash matches the cached value, the file hasn't changed.
fn compute_file_hash(path: &std::path::Path) -> u64 {
    let mut file = fs::File::open(path).expect("Failed to open DLL for hash computation");
    let mut hasher = DefaultHasher::new();
    let mut buf = [0u8; 8192];
    loop {
        let bytes_read = file
            .read(&mut buf)
            .expect("Failed to read DLL for hash computation");
        if bytes_read == 0 {
            break;
        }
        hasher.write(&buf[..bytes_read]);
    }
    hasher.finish()
}

/// Load previously cached DLL hashes from a simple text file format: `filename=hexhash\n`.
/// Returns an empty map if no cache file exists yet (first build or cache cleared).
fn load_hash_cache(path: &std::path::Path) -> HashMap<String, u64> {
    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return HashMap::new(),
    };

    let mut hashes = HashMap::new();
    for line in content.lines() {
        if let Some((name, hash_str)) = line.split_once('=') {
            if let Ok(hash) = u64::from_str_radix(hash_str.trim(), 16) {
                hashes.insert(name.to_string(), hash);
            }
        }
    }
    hashes
}

/// Save DLL hashes to a cache file for comparison on the next build.
/// Format: `filename=016x-hex-hash\n` (sorted by filename for deterministic output).
fn save_hash_cache(path: &std::path::Path, hashes: &HashMap<String, u64>) {
    let mut content = String::new();
    let mut sorted: Vec<_> = hashes.iter().collect();
    sorted.sort_by_key(|(k, _)| *k);
    for (name, hash) in &sorted {
        content.push_str(&format!("{}={:016x}\n", name, hash));
    }
    let _ = fs::write(path, content);
}

/// `ffmpeg-next` links `FFmpeg` DLLs dynamically on Windows. The loader must find them next to the
/// `.exe` or on `PATH`, otherwise the process exits with **`STATUS_DLL_NOT_FOUND` (0xc0000135)**
/// before `main`.
///
/// Copies every `*.dll` from the `FFmpeg` SDK `bin` directory into `target/<profile>/`.
///
/// Uses hash-based change detection: each source DLL is hashed and compared against a cached hash
/// from the previous build. DLLs are only re-copied when their content actually changes, saving
/// ~20-40 MB of I/O on every `cargo build` that has no FFmpeg changes.
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
    let ffmpeg_bin_dir = std::env::var("FFMPEG_DIR").map_or_else(
        |_| manifest_dir.join("ffmpeg_dev").join("sdk").join("bin"),
        |dir| PathBuf::from(dir).join("bin"),
    );

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

    let deps_dir = profile_dir.join("deps");
    let _ = fs::create_dir_all(&profile_dir);
    let _ = fs::create_dir_all(&deps_dir);

    // Load cached hashes from previous build for change detection
    let cache_path = profile_dir.join(".ffmpeg_dll_hashes");
    let cached_hashes = load_hash_cache(&cache_path);

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
    let mut skipped = 0usize;
    let mut new_hashes = HashMap::new();

    for entry in entries.flatten() {
        let path = entry.path();
        if path
            .extension()
            .and_then(|s| s.to_str())
            .is_none_or(|e| !e.eq_ignore_ascii_case("dll"))
        {
            continue;
        }
        let Some(name) = path.file_name().map(PathBuf::from) else {
            continue;
        };
        let name_str = name.to_string_lossy().to_string();

        // Track individual DLL paths so cargo only re-runs this script when a DLL actually changes
        println!("cargo:rerun-if-changed={}", path.display());

        // Compute hash of source DLL for change detection
        let source_hash = compute_file_hash(&path);
        new_hashes.insert(name_str.clone(), source_hash);

        // Check whether the source content has changed since last build
        let source_unchanged = cached_hashes.get(&name_str) == Some(&source_hash);

        // Main binary (e.g. liteclip.exe) loads DLLs from target/debug/.
        for dest_dir in [&profile_dir, &deps_dir] {
            let destination = dest_dir.join(&name);

            // Skip if source content is unchanged and destination already exists.
            // This avoids unnecessary I/O and warnings when DLL is locked by a running process.
            if source_unchanged && destination.exists() {
                skipped += 1;
                continue;
            }

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

    // Persist hashes so next build can skip unchanged DLLs
    save_hash_cache(&cache_path, &new_hashes);

    if copied == 0 && skipped == 0 {
        println!(
            "cargo:warning=liteclip: no .dll files were copied from {}. \
Add FFmpeg shared libraries there (or set FFMPEG_DIR) to fix 0xc0000135 at startup.",
            ffmpeg_bin_dir.display()
        );
    } else if failed > 0 {
        println!(
            "cargo:warning=liteclip: {failed} FFmpeg DLL copy operation(s) failed (see warnings above)."
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
