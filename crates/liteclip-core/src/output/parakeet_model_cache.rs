//! Download NVIDIA Parakeet CTC ONNX assets into the app cache (Hugging Face `onnx-community` bundle).

use anyhow::{bail, Context, Result};
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use tracing::info;
use zip::ZipArchive;

/// Hugging Face repo: ONNX Community Parakeet CTC 0.6B (matches `parakeet-rs` docs).
const HF_REPO_BASE: &str =
    "https://huggingface.co/onnx-community/parakeet-ctc-0.6b-ONNX/resolve/main";

/// Files required by `parakeet_rs::Parakeet::from_pretrained`.
///
/// Use the lighter Q4 ONNX variant by default to keep model initialization responsive.
/// Upstream stores ONNX payloads under `onnx/`; we mirror model files at cache root.
const REQUIRED_FILES: &[(&str, &str)] = &[
    ("config.json", "config.json"),
    ("preprocessor_config.json", "preprocessor_config.json"),
    ("tokenizer.json", "tokenizer.json"),
    ("tokenizer_config.json", "tokenizer_config.json"),
    ("special_tokens_map.json", "special_tokens_map.json"),
    ("onnx/model_q4.onnx", "model_q4.onnx"),
    ("onnx/model_q4.onnx_data", "model_q4.onnx_data"),
];

/// Progress while downloading ONNX assets (for UI). Throttled in the downloader.
#[derive(Debug, Clone)]
pub struct ParakeetModelDownloadProgress {
    pub file_index: usize,
    pub files_total: usize,
    pub filename: String,
    pub bytes_received: u64,
    pub bytes_total: Option<u64>,
}

impl ParakeetModelDownloadProgress {
    /// Overall 0..1 across all files (best effort if `bytes_total` is unknown mid-file).
    #[must_use]
    pub fn overall_fraction(&self) -> f32 {
        let n = self.files_total.max(1) as f32;
        let within_file = if let Some(t) = self.bytes_total.filter(|&t| t > 0) {
            (self.bytes_received as f64 / t as f64).min(1.0) as f32
        } else {
            0.5
        };
        ((self.file_index as f32 + within_file) / n).clamp(0.0, 1.0)
    }

    /// Single-line status for logs / UI.
    #[must_use]
    pub fn status_line(&self) -> String {
        match self.bytes_total {
            Some(t) if t > 0 => format!(
                "Downloading {} ({}/{}) — {} / {} MB",
                self.filename,
                self.file_index + 1,
                self.files_total,
                self.bytes_received / 1_048_576,
                t.div_ceil(1_048_576),
            ),
            _ => format!(
                "Downloading {} ({}/{}) — {} MB so far…",
                self.filename,
                self.file_index + 1,
                self.files_total,
                self.bytes_received / 1_048_576,
            ),
        }
    }
}

const PROGRESS_EMIT_INTERVAL_BYTES: u64 = 256 * 1024;

/// ORT 1.24.4 CPU build for Windows x64 — matches `ort` crate 2.0.0-rc.12 (`api-24`).
const ORT_DLL_URL: &str = "https://github.com/microsoft/onnxruntime/releases/download/v1.24.4/onnxruntime-win-x64-1.24.4.zip";
const ORT_DLL_ZIP_LIB_SUFFIX: &str = "lib/onnxruntime.dll";

fn cache_dir() -> Result<PathBuf> {
    let base = dirs::cache_dir().context("No cache directory (dirs::cache_dir)")?;
    Ok(base.join("liteclip").join("parakeet-ctc-0.6b-onnx"))
}

/// Ensure `onnxruntime.dll` (ORT 1.24.4, CPU build) is present in the Parakeet cache directory.
///
/// On first call this downloads the ~150 MB ORT release zip from GitHub, extracts only the DLL,
/// and deletes the zip. Subsequent calls return immediately because the DLL already exists.
/// Returns the absolute path to `onnxruntime.dll`.
pub(crate) fn ensure_ort_dylib_cached() -> Result<PathBuf> {
    let dir = cache_dir()?;
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("Failed to create Parakeet cache dir {}", dir.display()))?;

    let dll_path = dir.join("onnxruntime.dll");
    if dll_path.is_file() && dll_path.metadata().is_ok_and(|m| m.len() > 1_000_000) {
        return Ok(dll_path);
    }

    info!(
        url = ORT_DLL_URL,
        "Downloading ONNX Runtime DLL zip (one-time setup)"
    );
    let zip_tmp = dir.join("onnxruntime.zip.part");

    {
        let mut response = ureq::get(ORT_DLL_URL)
            .call()
            .map_err(|e| anyhow::anyhow!("HTTP GET failed for ORT DLL zip: {e}"))?;

        let bytes_total = response.body().content_length();
        let mut reader = response
            .body_mut()
            .with_config()
            .limit(1024 * 1024 * 1024)
            .reader();
        let mut out = File::create(&zip_tmp)
            .with_context(|| format!("Failed to create {}", zip_tmp.display()))?;
        let mut buf = [0u8; 64 * 1024];
        let mut received: u64 = 0;
        loop {
            let n = reader
                .read(&mut buf)
                .context("Failed to read ORT DLL zip")?;
            if n == 0 {
                break;
            }
            out.write_all(&buf[..n])
                .context("Failed to write ORT DLL zip")?;
            received += n as u64;
            if received % (10 * 1024 * 1024) < (64 * 1024) {
                info!(
                    received_mb = received / 1_048_576,
                    total_mb = bytes_total.map(|t| t / 1_048_576),
                    "Downloading ONNX Runtime zip"
                );
            }
        }
        info!(
            received_bytes = received,
            "ONNX Runtime zip download complete"
        );
    }

    info!("Extracting onnxruntime.dll from zip");
    {
        let zip_file = File::open(&zip_tmp)
            .with_context(|| format!("Failed to open {}", zip_tmp.display()))?;
        let mut archive =
            ZipArchive::new(zip_file).context("Failed to parse ONNX Runtime zip archive")?;

        let mut dll_idx = None;
        for i in 0..archive.len() {
            if let Ok(f) = archive.by_index(i) {
                if f.name()
                    .replace('\\', "/")
                    .ends_with(ORT_DLL_ZIP_LIB_SUFFIX)
                {
                    dll_idx = Some(i);
                    break;
                }
            }
        }
        let idx = dll_idx.with_context(|| {
            format!(
                "onnxruntime.dll not found in zip (looked for suffix '{ORT_DLL_ZIP_LIB_SUFFIX}')"
            )
        })?;

        let dll_tmp = dll_path.with_extension("part");
        {
            let mut entry = archive
                .by_index(idx)
                .context("Failed to open onnxruntime.dll entry in zip")?;
            let mut dest = File::create(&dll_tmp)
                .with_context(|| format!("Failed to create {}", dll_tmp.display()))?;
            std::io::copy(&mut entry, &mut dest).context("Failed to extract onnxruntime.dll")?;
        }
        std::fs::rename(&dll_tmp, &dll_path).with_context(|| {
            format!(
                "Failed to rename {} -> {}",
                dll_tmp.display(),
                dll_path.display()
            )
        })?;
    }

    let _ = std::fs::remove_file(&zip_tmp);
    info!(path = %dll_path.display(), "onnxruntime.dll cached successfully");
    Ok(dll_path)
}

/// Empty `trimmed_user_path` uses the app cache (download on first use); otherwise the path must exist.
pub fn resolve_parakeet_model_directory(trimmed_user_path: &str) -> Result<PathBuf> {
    if trimmed_user_path.is_empty() {
        ensure_parakeet_model_cached()
    } else {
        let p = PathBuf::from(trimmed_user_path);
        if p.is_dir() {
            ensure_legacy_onnx_layout_compat(&p)?;
            Ok(p)
        } else {
            anyhow::bail!(
                "Parakeet model folder does not exist or is not a directory: {}",
                p.display()
            );
        }
    }
}

/// Same as [`resolve_parakeet_model_directory`], but reports per-file download progress (and honours `cancel_flag`).
pub fn resolve_parakeet_model_directory_with_progress(
    trimmed_user_path: &str,
    progress_tx: &Sender<ParakeetModelDownloadProgress>,
    cancel_flag: &AtomicBool,
) -> Result<PathBuf> {
    if trimmed_user_path.is_empty() {
        ensure_parakeet_model_cached_with_progress(progress_tx, cancel_flag)
    } else {
        let p = PathBuf::from(trimmed_user_path);
        if p.is_dir() {
            ensure_legacy_onnx_layout_compat(&p)?;
            Ok(p)
        } else {
            anyhow::bail!(
                "Parakeet model folder does not exist or is not a directory: {}",
                p.display()
            );
        }
    }
}

/// Ensure the Parakeet ONNX model directory exists, downloading any missing files.
pub fn ensure_parakeet_model_cached() -> Result<PathBuf> {
    let (tx, _rx) = std::sync::mpsc::channel();
    let cancel = AtomicBool::new(false);
    ensure_parakeet_model_cached_with_progress(&tx, &cancel)
}

/// Ensure the Parakeet ONNX model directory exists, downloading any missing files with progress updates.
pub fn ensure_parakeet_model_cached_with_progress(
    progress_tx: &Sender<ParakeetModelDownloadProgress>,
    cancel_flag: &AtomicBool,
) -> Result<PathBuf> {
    let dir = cache_dir()?;
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("Failed to create Parakeet cache dir {}", dir.display()))?;

    let files_total = REQUIRED_FILES.len();

    for (file_index, &(remote_name, local_name)) in REQUIRED_FILES.iter().enumerate() {
        if cancel_flag.load(Ordering::Relaxed) {
            bail!("Cancelled while downloading Parakeet model");
        }

        let dest = dir.join(local_name);
        if dest.exists() && dest.metadata().map(|m| m.len() > 0).unwrap_or(false) {
            continue;
        }

        if let Some(src) = legacy_source_for_cache_file(&dir, remote_name) {
            materialize_model_alias(&src, &dest)
                .with_context(|| format!("Failed to materialize {}", dest.display()))?;
            continue;
        }

        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create parent directory for {}", dest.display())
            })?;
        }

        let url = format!("{HF_REPO_BASE}/{remote_name}");
        info!(url = %url, "Downloading Parakeet ONNX asset");

        download_one_file(
            &url,
            &dest,
            file_index,
            files_total,
            local_name,
            progress_tx,
            cancel_flag,
        )
        .with_context(|| format!("download failed for {remote_name}"))?;
    }

    ensure_legacy_onnx_layout_compat(&dir)?;

    Ok(dir)
}

fn ensure_legacy_onnx_layout_compat(model_dir: &Path) -> Result<()> {
    for file in [
        "model_q4.onnx",
        "model_q4.onnx_data",
        "model.onnx",
        "model.onnx_data",
    ] {
        let dst = model_dir.join(file);
        if dst.exists() {
            continue;
        }
        let src = model_dir.join("onnx").join(file);
        if src.exists() {
            materialize_model_alias(&src, &dst).with_context(|| {
                format!(
                    "Failed to make {} available next to tokenizer.json",
                    dst.display()
                )
            })?;
        }
    }
    Ok(())
}

fn legacy_source_for_cache_file(model_dir: &Path, remote_name: &str) -> Option<PathBuf> {
    let suffix = remote_name.strip_prefix("onnx/")?;
    let src = model_dir.join("onnx").join(suffix);
    src.exists().then_some(src)
}

fn materialize_model_alias(src: &Path, dst: &Path) -> Result<()> {
    if dst.exists() {
        return Ok(());
    }
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent).with_context(|| {
            format!(
                "Failed to create parent directory for materialized model file {}",
                dst.display()
            )
        })?;
    }

    match std::fs::hard_link(src, dst) {
        Ok(()) => Ok(()),
        Err(_) => {
            std::fs::copy(src, dst).with_context(|| {
                format!("Failed to copy {} -> {}", src.display(), dst.display())
            })?;
            Ok(())
        }
    }
}

fn download_one_file(
    url: &str,
    dest: &std::path::Path,
    file_index: usize,
    files_total: usize,
    filename: &str,
    progress_tx: &Sender<ParakeetModelDownloadProgress>,
    cancel_flag: &AtomicBool,
) -> Result<()> {
    let mut response = ureq::get(url)
        .call()
        .map_err(|e| anyhow::anyhow!("HTTP GET failed for {url}: {e}"))?;

    let bytes_total = response.body().content_length();

    let tmp = dest.with_extension("part");
    let mut file = File::create(&tmp).with_context(|| format!("create {:?}", tmp))?;

    let mut reader = response
        .body_mut()
        .with_config()
        .limit(3 * 1024 * 1024 * 1024)
        .reader();

    let mut buf = [0u8; 64 * 1024];
    let mut received: u64 = 0;
    let mut since_emit: u64 = 0;

    let emit = |received: u64, progress_tx: &Sender<ParakeetModelDownloadProgress>| {
        let _ = progress_tx.send(ParakeetModelDownloadProgress {
            file_index,
            files_total,
            filename: filename.to_string(),
            bytes_received: received,
            bytes_total,
        });
    };

    emit(0, progress_tx);

    loop {
        if cancel_flag.load(Ordering::Relaxed) {
            drop(file);
            let _ = std::fs::remove_file(&tmp);
            bail!("Cancelled while downloading Parakeet model");
        }

        let n = reader
            .read(&mut buf)
            .with_context(|| format!("read body for {filename}"))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])
            .with_context(|| format!("write {:?} part", tmp))?;
        received += n as u64;
        since_emit += n as u64;

        if since_emit >= PROGRESS_EMIT_INTERVAL_BYTES {
            emit(received, progress_tx);
            since_emit = 0;
        }
    }

    emit(received, progress_tx);

    drop(file);
    std::fs::rename(&tmp, dest).with_context(|| format!("rename {:?} -> {:?}", tmp, dest))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn download_progress_overall_fraction_mid_file() {
        let p = ParakeetModelDownloadProgress {
            file_index: 2,
            files_total: 6,
            filename: "model.onnx".to_string(),
            bytes_received: 500,
            bytes_total: Some(1000),
        };
        let f = p.overall_fraction();
        assert!(f > 0.33 && f < 0.42, "expected ~0.417, got {f}");
    }
}
