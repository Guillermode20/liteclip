//! Burned-in subtitles on exported clips using NVIDIA Parakeet (ONNX) + FFmpeg `subtitles` filter.
//! Transcription runs on **kept-range audio** in the editor; export only burns prepared cues.

use anyhow::{bail, Context, Result};
use ffmpeg_next as ffmpeg;
use ort::session::builder::GraphOptimizationLevel;
use parakeet_rs::{ExecutionConfig, Parakeet, TimestampMode, Transcriber};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tracing::{info, warn};

use super::video_file::{
    ClipExportPhase, ClipExportRequest, ClipExportUpdate, ExportOutcome, PreparedSubtitleCue,
    PreparedSubtitles, SubtitleTranscribeProgress, TimeRange,
};
use crate::runtime::resolve_ffmpeg_executable;

const TARGET_SAMPLE_RATE: u32 = 16_000;
/// Chunk work for more responsive progress and lower peak compute time per step.
const CHUNK_SAMPLES: usize = TARGET_SAMPLE_RATE as usize * 30;

struct CachedParakeet {
    path: PathBuf,
    model: Parakeet,
}

static MODEL_CACHE: OnceLock<Mutex<Option<CachedParakeet>>> = OnceLock::new();

fn seek_to_seconds(input_ctx: &mut ffmpeg::format::context::Input, position_secs: f64) {
    let ts = (position_secs.max(0.0) * f64::from(ffmpeg::ffi::AV_TIME_BASE)).round() as i64;
    let _ = input_ctx.seek(ts, ..);
}

/// Transcribe audio for the concatenated **export** timeline (matches muxed output order/duration).
pub(crate) fn transcribe_kept_ranges_parakeet(
    input_path: &Path,
    keep_ranges: &[TimeRange],
    model_dir: &Path,
    progress_tx: &Sender<SubtitleTranscribeProgress>,
    cancel_flag: &Arc<AtomicBool>,
) -> Result<Vec<PreparedSubtitleCue>> {
    if cancel_flag.load(Ordering::Relaxed) {
        bail!("Cancelled");
    }
    let _ = progress_tx.send(SubtitleTranscribeProgress {
        fraction: 0.0,
        message: "Decoding audio for your kept segments".to_string(),
    });

    let samples =
        decode_audio_mono_f32_16k_for_ranges(input_path, keep_ranges, progress_tx, cancel_flag)
            .context("Failed to decode audio for transcription")?;

    if samples.is_empty() {
        bail!("No audio decoded for the current cuts; cannot transcribe");
    }

    let _ = progress_tx.send(SubtitleTranscribeProgress {
        fraction: 0.12,
        message: "Loading Parakeet model".to_string(),
    });
    let model_path = if model_dir.join("model_q4.onnx").is_file() {
        model_dir.join("model_q4.onnx")
    } else {
        model_dir.to_path_buf()
    };
    let q4_data = model_dir.join("model_q4.onnx_data");
    let base_model = model_dir.join("model.onnx");
    let base_data = model_dir.join("model.onnx_data");
    info!(
        model_dir = %model_dir.display(),
        selected_model_path = %model_path.display(),
        q4_model_exists = model_dir.join("model_q4.onnx").is_file(),
        q4_data_exists = q4_data.is_file(),
        base_model_exists = base_model.is_file(),
        base_data_exists = base_data.is_file(),
        "Initializing Parakeet model"
    );
    if model_path.is_file() {
        match std::fs::metadata(&model_path) {
            Ok(meta) => info!(
                model_file = %model_path.display(),
                size_bytes = meta.len(),
                "Parakeet model file metadata"
            ),
            Err(err) => warn!(
                model_file = %model_path.display(),
                error = %err,
                "Failed to stat selected Parakeet model file"
            ),
        }
    }
    if q4_data.is_file() {
        if let Ok(meta) = std::fs::metadata(&q4_data) {
            info!(
                model_data_file = %q4_data.display(),
                size_bytes = meta.len(),
                "Parakeet model data file metadata"
            );
        }
    }

    let model_cache = MODEL_CACHE.get_or_init(|| Mutex::new(None));
    let mut guard = model_cache.lock().unwrap_or_else(|e| e.into_inner());
    let cache_hit = guard.as_ref().is_some_and(|c| c.path == model_path);
    if !cache_hit {
        let dll_path = super::parakeet_model_cache::ensure_ort_dylib_cached()
            .context("Failed to obtain ONNX Runtime DLL")?;
        let _ = progress_tx.send(SubtitleTranscribeProgress {
            fraction: 0.13,
            message: "Initializing ONNX Runtime".to_string(),
        });
        match ort::init_from(&dll_path) {
            Ok(env) => {
                env.commit();
                info!(dll = %dll_path.display(), "ORT initialized from managed DLL");
            }
            Err(e) => {
                info!("ort::init_from returned error (may already be initialized): {e}");
            }
        }

        let exec_config = ExecutionConfig::default()
            .with_intra_threads(1)
            .with_inter_threads(1)
            .with_custom_configure(|builder| {
                Ok(builder.with_optimization_level(GraphOptimizationLevel::Disable)?)
            });
        info!(
            intra_threads = 1,
            inter_threads = 1,
            optimization_level = "Disable",
            "Loading Parakeet model (no graph optimization)"
        );

        let load_started = Instant::now();
        info!("Calling Parakeet::from_pretrained");
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let progress_tx_watch = progress_tx.clone();
        let model_path_for_watch = model_path.clone();
        let watchdog = std::thread::spawn(move || {
            let mut last_reported_secs = 0u64;
            loop {
                match stop_rx.recv_timeout(Duration::from_secs(5)) {
                    Ok(_) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        let elapsed_secs = load_started.elapsed().as_secs();
                        if elapsed_secs >= last_reported_secs + 5 {
                            last_reported_secs = elapsed_secs;
                            info!(
                                model = %model_path_for_watch.display(),
                                elapsed_secs,
                                "Parakeet model still loading"
                            );
                            let _ = progress_tx_watch.send(SubtitleTranscribeProgress {
                                fraction: 0.12,
                                message: format!("Loading Parakeet model ({elapsed_secs}s)"),
                            });
                        }
                    }
                }
            }
        });

        let parakeet_result = Parakeet::from_pretrained(&model_path, Some(exec_config));
        let _ = stop_tx.send(());
        let _ = watchdog.join();
        let model = parakeet_result.map_err(|e| {
            let elapsed_secs = load_started.elapsed().as_secs_f32();
            anyhow::anyhow!(
                "Failed to load Parakeet model from {} after {:.2}s: {e}",
                model_path.display(),
                elapsed_secs
            )
        })?;
        info!(
            elapsed_secs = load_started.elapsed().as_secs_f32(),
            "Parakeet model initialized and cached"
        );
        *guard = Some(CachedParakeet {
            path: model_path.clone(),
            model,
        });
    } else {
        info!("Reusing cached Parakeet model");
        let _ = progress_tx.send(SubtitleTranscribeProgress {
            fraction: 0.15,
            message: "Parakeet model ready (cached)".to_string(),
        });
    }
    let parakeet = &mut guard.as_mut().expect("cache was just populated").model;

    let chunk_secs = CHUNK_SAMPLES as f32 / TARGET_SAMPLE_RATE as f32;
    let mut all_cues: Vec<(f32, f32, String)> = Vec::new();

    let total_chunks = samples.len().div_ceil(CHUNK_SAMPLES).max(1);
    for (chunk_idx, chunk) in samples.chunks(CHUNK_SAMPLES).enumerate() {
        if cancel_flag.load(Ordering::Relaxed) {
            bail!("Cancelled during transcription");
        }

        let started_chunks = chunk_idx as f32 / total_chunks as f32;
        let _ = progress_tx.send(SubtitleTranscribeProgress {
            fraction: (0.15 + 0.8 * started_chunks).clamp(0.0, 0.95),
            message: format!("Transcribing chunk {}/{}", chunk_idx + 1, total_chunks),
        });
        let time_offset = chunk_idx as f32 * chunk_secs;
        let tr = parakeet
            .transcribe_samples(
                chunk.to_vec(),
                TARGET_SAMPLE_RATE,
                1,
                Some(TimestampMode::Words),
            )
            .map_err(|e| anyhow::anyhow!("Parakeet transcription failed: {e}"))?;

        for t in tr.tokens {
            let start = time_offset + t.start;
            let end = time_offset + t.end;
            let text = t.text.trim();
            if text.is_empty() {
                continue;
            }
            all_cues.push((start, end, text.to_string()));
        }

        let done_chunks = chunk_idx + 1;
        let frac = (done_chunks as f32 / total_chunks as f32).min(1.0);
        let _ = progress_tx.send(SubtitleTranscribeProgress {
            fraction: 0.15 + 0.8 * frac,
            message: format!("Transcribing ({done_chunks}/{total_chunks})"),
        });
    }

    let lines = merge_cues_into_lines(all_cues, 14);
    Ok(lines
        .into_iter()
        .map(|(start_secs, end_secs, text)| PreparedSubtitleCue {
            start_secs,
            end_secs,
            text,
        })
        .collect())
}

fn decode_audio_mono_f32_16k_for_ranges(
    path: &Path,
    ranges: &[TimeRange],
    progress_tx: &Sender<SubtitleTranscribeProgress>,
    cancel_flag: &Arc<AtomicBool>,
) -> Result<Vec<f32>> {
    if ranges.is_empty() {
        return Ok(Vec::new());
    }

    let mut input = ffmpeg::format::input(path)
        .with_context(|| format!("Failed to open {:?} for audio decode", path))?;

    let audio_stream = input
        .streams()
        .best(ffmpeg::media::Type::Audio)
        .context("No audio stream in file")?;
    let audio_idx = audio_stream.index();
    let audio_tb = audio_stream.time_base();

    let mut all_samples: Vec<f32> = Vec::new();

    let total_ranges = ranges.len().max(1);
    for (range_idx, range) in ranges.iter().enumerate() {
        if cancel_flag.load(Ordering::Relaxed) {
            bail!("Cancelled during audio decode");
        }
        let range_msg_index = range_idx + 1;
        let _ = progress_tx.send(SubtitleTranscribeProgress {
            fraction: 0.02 + 0.06 * (range_idx as f32 / total_ranges as f32),
            message: format!("Decoding audio for kept segment {range_msg_index}/{total_ranges}"),
        });

        seek_to_seconds(&mut input, range.start_secs);

        let a_stream = input
            .stream(audio_idx)
            .with_context(|| format!("missing audio stream {audio_idx} after seek"))?;
        let mut decoder = ffmpeg::codec::context::Context::from_parameters(a_stream.parameters())
            .context("Audio codec parameters")?
            .decoder()
            .audio()
            .context("Not an audio decoder")?;

        let mut resampler = ffmpeg::software::resampling::Context::get(
            decoder.format(),
            decoder.channel_layout(),
            decoder.rate(),
            ffmpeg::format::Sample::F32(ffmpeg::format::sample::Type::Planar),
            ffmpeg::channel_layout::ChannelLayout::MONO,
            TARGET_SAMPLE_RATE,
        )
        .context("Failed to create audio resampler for Parakeet")?;

        let mut decoded = ffmpeg::util::frame::audio::Audio::empty();
        let mut resampled = ffmpeg::util::frame::audio::Audio::empty();

        let mut stop = false;
        for (stream, packet) in input.packets() {
            if cancel_flag.load(Ordering::Relaxed) {
                bail!("Cancelled during audio decode");
            }
            if stream.index() != audio_idx {
                continue;
            }
            if let Some(pts) = packet.pts() {
                let pts_secs = pts as f64 * f64::from(audio_tb.numerator())
                    / f64::from(audio_tb.denominator());
                if pts_secs > range.end_secs + 0.5 {
                    stop = true;
                }
            }

            decoder.send_packet(&packet)?;
            while decoder.receive_frame(&mut decoded).is_ok() {
                let pts_secs = decoded
                    .timestamp()
                    .map(|ts| {
                        ts as f64 * f64::from(audio_tb.numerator())
                            / f64::from(audio_tb.denominator())
                    })
                    .unwrap_or(0.0);
                if pts_secs >= range.end_secs {
                    continue;
                }
                if pts_secs < range.start_secs {
                    continue;
                }
                resampler.run(&decoded, &mut resampled)?;
                append_planar_f32_mono(&resampled, &mut all_samples);
            }
            if stop {
                break;
            }
        }

        decoder.send_eof()?;
        while decoder.receive_frame(&mut decoded).is_ok() {
            let pts_secs = decoded
                .timestamp()
                .map(|ts| {
                    ts as f64 * f64::from(audio_tb.numerator()) / f64::from(audio_tb.denominator())
                })
                .unwrap_or(0.0);
            if pts_secs >= range.end_secs {
                continue;
            }
            if pts_secs < range.start_secs {
                continue;
            }
            resampler.run(&decoded, &mut resampled)?;
            append_planar_f32_mono(&resampled, &mut all_samples);
        }
    }

    let _ = progress_tx.send(SubtitleTranscribeProgress {
        fraction: 0.1,
        message: "Audio decode complete; starting transcription".to_string(),
    });

    Ok(all_samples)
}

/// After a successful mux/export, burn editor-prepared subtitles into the file (no Parakeet call).
pub(crate) fn finalize_export_with_parakeet_subtitles(
    request: &ClipExportRequest,
    prior: ExportOutcome,
    progress_tx: &Sender<ClipExportUpdate>,
    cancel_flag: &Arc<AtomicBool>,
) -> Result<ExportOutcome> {
    match prior {
        ExportOutcome::Cancelled => Ok(prior),
        ExportOutcome::Finished(path) => {
            if !request.burn_auto_subtitles || !request.metadata.has_audio {
                return Ok(ExportOutcome::Finished(path));
            }
            let Some(prep) = request.prepared_subtitles.as_ref() else {
                bail!(
                    "Burn-in subtitles are enabled but no subtitle cues were prepared. Generate subtitles in the editor first."
                );
            };
            apply_prepared_burn(&path, prep, progress_tx, cancel_flag)
        }
    }
}

fn apply_prepared_burn(
    video_path: &Path,
    prep: &PreparedSubtitles,
    progress_tx: &Sender<ClipExportUpdate>,
    cancel_flag: &Arc<AtomicBool>,
) -> Result<ExportOutcome> {
    if cancel_flag.load(Ordering::Relaxed) {
        return Ok(ExportOutcome::Cancelled);
    }

    let lines: Vec<(f32, f32, String)> = prep
        .cues
        .iter()
        .filter(|c| !c.text.trim().is_empty())
        .map(|c| (c.start_secs, c.end_secs, c.text.clone()))
        .collect();

    if lines.is_empty() {
        warn!("Prepared subtitles are empty; skipping burn-in");
        return Ok(ExportOutcome::Finished(video_path.to_path_buf()));
    }

    let srt = build_srt(&lines);
    let force_style = force_style_string(prep);

    let temp_dir = std::env::temp_dir().join(format!(
        "liteclip-subs-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_millis()
    ));
    std::fs::create_dir_all(&temp_dir)
        .with_context(|| format!("Failed to create temp dir {:?}", temp_dir))?;
    let srt_path = temp_dir.join("export_subtitles.srt");
    let mut f = std::fs::File::create(&srt_path).context("Failed to write temporary SRT")?;
    f.write_all(srt.as_bytes())
        .context("Failed to write SRT contents")?;
    drop(f);

    if cancel_flag.load(Ordering::Relaxed) {
        let _ = std::fs::remove_dir_all(&temp_dir);
        return Ok(ExportOutcome::Cancelled);
    }

    let _ = progress_tx.send(ClipExportUpdate::Progress {
        phase: ClipExportPhase::BurningSubtitles,
        fraction: 0.0,
        message: "Burning subtitles into video".to_string(),
    });

    let tmp_out = temp_dir.join("with_subs.mp4");
    burn_subtitles_sdk_then_cli_fallback(
        video_path,
        &srt_path,
        &tmp_out,
        Some(force_style.as_str()),
        cancel_flag,
    )?;

    replace_file(&tmp_out, video_path)?;

    let _ = std::fs::remove_dir_all(&temp_dir);

    info!(output = ?video_path, "Burned prepared subtitles into export");

    Ok(ExportOutcome::Finished(video_path.to_path_buf()))
}

fn force_style_string(prep: &PreparedSubtitles) -> String {
    format!(
        "PrimaryColour={},OutlineColour=&H40000000,BorderStyle=1",
        prep.primary_colour_ass
    )
}

/// Prefer linked libavfilter (`ffmpeg-next`); fall back to `ffmpeg` CLI if the filter graph fails.
fn burn_subtitles_sdk_then_cli_fallback(
    video_in: &Path,
    srt_path: &Path,
    video_out: &Path,
    force_style: Option<&str>,
    cancel_flag: &Arc<AtomicBool>,
) -> Result<()> {
    if cancel_flag.load(Ordering::Relaxed) {
        bail!("Cancelled before subtitle burn");
    }
    let _ = std::fs::remove_file(video_out);
    match super::subtitle_burn_sdk::burn_subtitles_with_filter_graph(
        video_in,
        srt_path,
        video_out,
        force_style,
    ) {
        Ok(()) => Ok(()),
        Err(e) => {
            warn!(
                error = %e,
                "Linked FFmpeg subtitle burn failed; retrying with ffmpeg CLI"
            );
            let _ = std::fs::remove_file(video_out);
            run_ffmpeg_subtitle_burn_cli(video_in, srt_path, video_out, force_style, cancel_flag)
        }
    }
}

fn replace_file(from: &Path, to: &Path) -> Result<()> {
    match std::fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(_) => {
            std::fs::copy(from, to).context("Failed to copy burned subtitle output over export")?;
            let _ = std::fs::remove_file(from);
            Ok(())
        }
    }
}

fn append_planar_f32_mono(frame: &ffmpeg::util::frame::Audio, out: &mut Vec<f32>) {
    let n = frame.samples();
    if n == 0 {
        return;
    }
    let ch = frame.channels() as usize;
    if ch == 1 {
        let plane0 = frame.data(0);
        let ptr = plane0.as_ptr() as *const f32;
        let slice = unsafe { std::slice::from_raw_parts(ptr, n) };
        out.extend_from_slice(slice);
        return;
    }
    for i in 0..n {
        let mut sum = 0.0f32;
        for c in 0..ch {
            let plane = frame.data(c);
            let ptr = plane.as_ptr() as *const f32;
            let slice = unsafe { std::slice::from_raw_parts(ptr, n) };
            sum += slice[i];
        }
        out.push(sum / ch as f32);
    }
}

/// Merge word-level cues into subtitle lines (max `max_words` words per cue).
fn merge_cues_into_lines(
    cues: Vec<(f32, f32, String)>,
    max_words: usize,
) -> Vec<(f32, f32, String)> {
    if cues.is_empty() {
        return Vec::new();
    }
    let max_words = max_words.max(1);
    let mut out: Vec<(f32, f32, String)> = Vec::new();
    let mut batch_start = cues[0].0;
    let mut batch_end = cues[0].1;
    let mut words: Vec<String> = Vec::new();

    for (start, end, w) in cues {
        if !words.is_empty() && words.len() >= max_words {
            out.push((batch_start, batch_end, words.join(" ")));
            words.clear();
            batch_start = start;
            batch_end = end;
            words.push(w);
        } else {
            if words.is_empty() {
                batch_start = start;
            }
            batch_end = end;
            words.push(w);
        }
    }
    if !words.is_empty() {
        out.push((batch_start, batch_end, words.join(" ")));
    }
    out
}

fn format_srt_timestamp(seconds: f32) -> String {
    let s = seconds.max(0.0);
    let h = (s / 3600.0) as u32;
    let m = ((s % 3600.0) / 60.0) as u32;
    let sec = (s % 60.0).floor() as u32;
    let ms = ((s - s.floor()) * 1000.0).round() as u32;
    format!("{h:02}:{m:02}:{sec:02},{ms:03}")
}

fn build_srt(lines: &[(f32, f32, String)]) -> String {
    let mut s = String::new();
    let mut index = 1usize;
    for (start, end, text) in lines {
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        let start_ts = *start;
        let end_ts = (*end).max(start_ts + 0.1);
        s.push_str(&format!(
            "{index}\n{} --> {}\n{text}\n\n",
            format_srt_timestamp(start_ts),
            format_srt_timestamp(end_ts),
        ));
        index += 1;
    }
    s
}

fn run_ffmpeg_subtitle_burn_cli(
    video_in: &Path,
    srt_path: &Path,
    video_out: &Path,
    force_style: Option<&str>,
    cancel_flag: &Arc<AtomicBool>,
) -> Result<()> {
    if cancel_flag.load(Ordering::Relaxed) {
        bail!("Cancelled before subtitle burn");
    }
    let vf = subtitles_filter_arg(srt_path, force_style)?;
    let ffmpeg_exe = resolve_ffmpeg_executable();
    info!(?ffmpeg_exe, "Running FFmpeg to burn subtitles");

    let out = Command::new(&ffmpeg_exe)
        .arg("-nostdin")
        .arg("-y")
        .arg("-i")
        .arg(video_in)
        .arg("-vf")
        .arg(&vf)
        .arg("-c:v")
        .arg("libx265")
        .arg("-crf")
        .arg("20")
        .arg("-preset")
        .arg("medium")
        .arg("-c:a")
        .arg("copy")
        .arg("-movflags")
        .arg("+faststart")
        .arg(video_out)
        .output()
        .with_context(|| format!("Failed to run FFmpeg ({})", ffmpeg_exe.display()))?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        if stderr.contains("libx265") || stderr.contains("Unknown encoder") {
            warn!("libx265 not available; retrying subtitle burn with libx264");
            return run_ffmpeg_subtitle_burn_cli_h264(
                video_in,
                srt_path,
                video_out,
                force_style,
                cancel_flag,
            );
        }
        bail!("FFmpeg subtitle burn failed: {stderr}");
    }

    Ok(())
}

fn run_ffmpeg_subtitle_burn_cli_h264(
    video_in: &Path,
    srt_path: &Path,
    video_out: &Path,
    force_style: Option<&str>,
    cancel_flag: &Arc<AtomicBool>,
) -> Result<()> {
    if cancel_flag.load(Ordering::Relaxed) {
        bail!("Cancelled before subtitle burn (H.264)");
    }
    let vf = subtitles_filter_arg(srt_path, force_style)?;
    let ffmpeg_exe = resolve_ffmpeg_executable();
    let out = Command::new(&ffmpeg_exe)
        .arg("-nostdin")
        .arg("-y")
        .arg("-i")
        .arg(video_in)
        .arg("-vf")
        .arg(&vf)
        .arg("-c:v")
        .arg("libx264")
        .arg("-crf")
        .arg("18")
        .arg("-preset")
        .arg("medium")
        .arg("-c:a")
        .arg("copy")
        .arg("-movflags")
        .arg("+faststart")
        .arg(video_out)
        .output()
        .with_context(|| format!("Failed to run FFmpeg ({})", ffmpeg_exe.display()))?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        bail!("FFmpeg subtitle burn (H.264 fallback) failed: {stderr}");
    }
    Ok(())
}

/// Build `subtitles=` filter argument with Windows-safe path escaping.
fn subtitles_filter_arg(srt_path: &Path, force_style: Option<&str>) -> Result<String> {
    let abs: PathBuf = srt_path
        .canonicalize()
        .unwrap_or_else(|_| srt_path.to_path_buf());
    let p = abs.to_string_lossy().replace('\\', "/");
    let escaped = p.replace(':', "\\:");
    let mut spec = format!("subtitles={escaped}");
    if let Some(fs) = force_style {
        let fs_escaped = fs.replace('\'', "\\'");
        spec.push_str(&format!(":force_style='{fs_escaped}'"));
    }
    Ok(spec)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srt_timestamp_format() {
        assert_eq!(format_srt_timestamp(0.0), "00:00:00,000");
        assert_eq!(format_srt_timestamp(3661.5), "01:01:01,500");
    }
}
