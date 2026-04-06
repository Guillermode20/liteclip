//! Clip export using ffmpeg-next filter graphs (SDK-based, no CLI subprocess).

use anyhow::{Context, Result};
use ffmpeg_next as ffmpeg;
use libc::c_int;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::Sender,
    Arc,
};
use std::time::Instant;
use tracing::{info, warn};

use super::video_file::{
    ClipExportPhase, ClipExportRequest, ClipExportUpdate, CropRect, ExportAttemptResult,
    ExportOutcome, ExportVideoEncoder,
};

const AAC_FRAME_SAMPLES: i64 = 1024;
const INVALID_DURATION: i64 = i64::MIN;

// ---------------------------------------------------------------------------
// Adaptive post-processing filters
// ---------------------------------------------------------------------------

/// Computed filter strengths for the adaptive post-processing filter chain.
///
/// These values are derived from the bits-per-pixel metric and determine
/// how aggressively each filter processes frames at a given quality level.
#[derive(Debug, Clone, Copy)]
struct AdaptiveFilterStrengths {
    /// Luma sharpening amount for the `unsharp` filter (0.0 – 2.0).
    unsharp_luma: f64,
    /// Sharpening kernel half-width (3 or 5).
    unsharp_size_x: u32,
    unsharp_size_y: u32,
    /// Chroma sharpening amount (typically lower than luma).
    unsharp_chroma: f64,
    /// Spatial denoising strength for `hqdn3d` luma plane (0.0 – 10.0).
    hqdn3d_luma_spatial: f64,
    /// Temporal denoising strength for `hqdn3d` luma plane (0.0 – 8.0).
    hqdn3d_luma_temporal: f64,
    /// Spatial denoising strength for `hqdn3d` chroma planes.
    hqdn3d_chroma_spatial: f64,
    /// Temporal denoising strength for `hqdn3d` chroma planes.
    hqdn3d_chroma_temporal: f64,
    /// Enable `pp` (postprocessing) deblock+dering filter. Best for blocking
    /// artifacts at low BPP where block boundaries are clearly visible.
    pp_enable: bool,
    /// Contrast boost via `eq` filter (1.0 = no change; >1.0 = more contrast).
    /// Applied at very low BPP to partially compensate for washed-out appearance.
    eq_contrast: f64,
}

/// Bits-per-pixel thresholds for adaptive filter strength selection.
/// HIGH  (≥0.15): well-encoded — only gentle sharpening, no deblock needed.
/// MEDIUM (0.06–0.15): moderate compression — light denoise + sharpen.
/// LOW   (<0.06): heavy compression — full deblock/denoise/sharpen/contrast.
const HIGH_BPP_THRESHOLD: f64 = 0.15;
const MEDIUM_BPP_THRESHOLD: f64 = 0.06;
/// Below this threshold the `pp` deblock filter is activated.
const DEBLOCK_BPP_THRESHOLD: f64 = 0.10;
/// Below this threshold mild contrast boosting via `eq` is applied.
const EQ_CONTRAST_BPP_THRESHOLD: f64 = 0.04;

fn compute_filter_strengths(
    video_bitrate_kbps: u32,
    width: u32,
    height: u32,
    fps: f64,
) -> AdaptiveFilterStrengths {
    let pixel_rate = f64::from(width) * f64::from(height) * fps.max(1.0);
    let bpp = (f64::from(video_bitrate_kbps) * 1000.0) / pixel_rate;

    let pp_enable = bpp < DEBLOCK_BPP_THRESHOLD;
    // Mild contrast lift at very low BPP: 1.0 (none) → 1.08 (max) below EQ threshold.
    let eq_contrast = if bpp < EQ_CONTRAST_BPP_THRESHOLD {
        1.0 + 0.08 * (1.0 - (bpp / EQ_CONTRAST_BPP_THRESHOLD).clamp(0.0, 1.0))
    } else {
        1.0
    };

    if bpp >= HIGH_BPP_THRESHOLD {
        AdaptiveFilterStrengths {
            unsharp_luma: 0.3,
            unsharp_size_x: 3,
            unsharp_size_y: 3,
            unsharp_chroma: 0.0,
            hqdn3d_luma_spatial: 0.0,
            hqdn3d_luma_temporal: 0.0,
            hqdn3d_chroma_spatial: 0.0,
            hqdn3d_chroma_temporal: 0.0,
            pp_enable,
            eq_contrast,
        }
    } else if bpp >= MEDIUM_BPP_THRESHOLD {
        let t = ((bpp - MEDIUM_BPP_THRESHOLD) / (HIGH_BPP_THRESHOLD - MEDIUM_BPP_THRESHOLD))
            .clamp(0.0, 1.0);
        AdaptiveFilterStrengths {
            unsharp_luma: 0.5 + 0.5 * (1.0 - t),
            unsharp_size_x: 5,
            unsharp_size_y: 5,
            unsharp_chroma: 0.15 * (1.0 - t),
            hqdn3d_luma_spatial: 2.0 + 2.0 * (1.0 - t),
            hqdn3d_luma_temporal: 1.5 + 2.0 * (1.0 - t),
            hqdn3d_chroma_spatial: 1.5 + 1.5 * (1.0 - t),
            hqdn3d_chroma_temporal: 1.0 + 1.5 * (1.0 - t),
            pp_enable,
            eq_contrast,
        }
    } else {
        let t = (bpp / MEDIUM_BPP_THRESHOLD).clamp(0.0, 1.0);
        AdaptiveFilterStrengths {
            unsharp_luma: 0.6 + 0.3 * t,
            unsharp_size_x: 5,
            unsharp_size_y: 5,
            unsharp_chroma: 0.15 * t,
            hqdn3d_luma_spatial: 4.0 + 2.0 * (1.0 - t),
            hqdn3d_luma_temporal: 3.0 + 2.0 * (1.0 - t),
            hqdn3d_chroma_spatial: 3.0 + 1.5 * (1.0 - t),
            hqdn3d_chroma_temporal: 2.0 + 1.5 * (1.0 - t),
            pp_enable,
            eq_contrast,
        }
    }
}

/// Wrapper around an FFmpeg filter graph that applies post-processing filters
/// to scaled YUV420P frames before encoding.
///
/// Filter chain (active filters depend on BPP):
///   `buffer → [pp] → [hqdn3d] → unsharp → [eq] → buffersink`
///
/// - `pp`     — H.264 postprocessing deblock + dering (spatial, no latency).
/// - `hqdn3d` — High-quality 3D denoise; temporal mode adds 1-frame latency
///   but process_into() drains the sink in a loop so no frames
///   are ever dropped.
/// - `unsharp`— Adaptive sharpen/blur to recover perceived detail.
/// - `eq`     — Mild contrast lift at very low BPP.
struct PostProcessFilterGraph {
    #[allow(dead_code)]
    graph: ffmpeg::filter::Graph,
    source: ffmpeg::filter::Context,
    sink: ffmpeg::filter::Context,
    filtered_frame: ffmpeg::frame::Video,
}

impl PostProcessFilterGraph {
    /// Build the adaptive post-processing filter graph.
    ///
    /// The graph takes YUV420P frames at `(width, height)` and outputs
    /// processed frames at the same resolution.
    fn new(width: u32, height: u32, fps: f64, strengths: AdaptiveFilterStrengths) -> Result<Self> {
        let mut graph = ffmpeg::filter::Graph::new();

        let args = format!(
            "video_size={width}x{height}:pix_fmt=0:time_base=1/{fps_int}:pixel_aspect=1/1",
            fps_int = fps.round() as u32,
        );

        let source_filter = ffmpeg::filter::find("buffer").context("buffer filter not found")?;
        let sink_filter =
            ffmpeg::filter::find("buffersink").context("buffersink filter not found")?;

        let mut source = graph
            .add(&source_filter, "in", &args)
            .context("Failed to add buffer filter to graph")?;
        let mut sink = graph
            .add(&sink_filter, "out", "")
            .context("Failed to add buffersink filter to graph")?;

        // pp (postprocessing) deblock + dering — spatial, zero extra latency.
        // ha/va = horizontal/vertical deblock; dr = dering.
        // Only inserted when BPP is below the deblock threshold.
        let pp_args = if strengths.pp_enable { "ha:va:dr" } else { "" };

        // hqdn3d denoise — luma_spatial:chroma_spatial:luma_tmp:chroma_tmp.
        // Temporal mode (luma_tmp/chroma_tmp > 0) buffers 1 frame but
        // process_into() loops on the sink so no frames are dropped.
        let hqdn3d_args = if strengths.hqdn3d_luma_spatial > 0.0 {
            format!(
                "{ls:.2}:{cs:.2}:{lt:.2}:{ct:.2}",
                ls = strengths.hqdn3d_luma_spatial,
                cs = strengths.hqdn3d_chroma_spatial,
                lt = strengths.hqdn3d_luma_temporal,
                ct = strengths.hqdn3d_chroma_temporal,
            )
        } else {
            String::new()
        };

        // unsharp — lx:ly:la:cx:cy:ca
        let unsharp_args = if strengths.unsharp_chroma > 0.0 {
            format!(
                "lx={}:ly={}:la={:.2}:cx={}:cy={}:ca={:.2}",
                strengths.unsharp_size_x,
                strengths.unsharp_size_y,
                strengths.unsharp_luma,
                strengths.unsharp_size_x,
                strengths.unsharp_size_y,
                strengths.unsharp_chroma,
            )
        } else {
            format!(
                "lx={}:ly={}:la={:.2}",
                strengths.unsharp_size_x, strengths.unsharp_size_y, strengths.unsharp_luma,
            )
        };

        // eq contrast boost — only active at very low BPP.
        let eq_args = if strengths.eq_contrast > 1.0 {
            format!("contrast={:.3}", strengths.eq_contrast)
        } else {
            String::new()
        };

        // Wire up the filter chain: source → [pp →] [hqdn3d →] unsharp → [eq →] sink
        let unsharp_filter = ffmpeg::filter::find("unsharp").context("unsharp filter not found")?;
        let mut unsharp_ctx = graph
            .add(&unsharp_filter, "unsharp", &unsharp_args)
            .context("Failed to add unsharp filter")?;

        // prev_ctx tracks the tail of the chain for sequential linking.
        let mut chain_tail: Option<ffmpeg::filter::Context> = None;

        if !pp_args.is_empty() {
            let pp_filter = ffmpeg::filter::find("pp").context("pp filter not found")?;
            let mut pp_ctx = graph
                .add(&pp_filter, "pp", pp_args)
                .context("Failed to add pp filter")?;
            source.link(0, &mut pp_ctx, 0);
            chain_tail = Some(pp_ctx);
        }

        if !hqdn3d_args.is_empty() {
            let hqdn3d_filter =
                ffmpeg::filter::find("hqdn3d").context("hqdn3d filter not found")?;
            let mut hqdn3d_ctx = graph
                .add(&hqdn3d_filter, "hqdn3d", &hqdn3d_args)
                .context("Failed to add hqdn3d filter")?;
            if let Some(ref mut tail) = chain_tail {
                tail.link(0, &mut hqdn3d_ctx, 0);
            } else {
                source.link(0, &mut hqdn3d_ctx, 0);
            }
            chain_tail = Some(hqdn3d_ctx);
        }

        // Link previous tail (or source if no prior filters) into unsharp.
        if let Some(ref mut tail) = chain_tail {
            tail.link(0, &mut unsharp_ctx, 0);
        } else {
            source.link(0, &mut unsharp_ctx, 0);
        }

        if !eq_args.is_empty() {
            let eq_filter = ffmpeg::filter::find("eq").context("eq filter not found")?;
            let mut eq_ctx = graph
                .add(&eq_filter, "eq", &eq_args)
                .context("Failed to add eq filter")?;
            unsharp_ctx.link(0, &mut eq_ctx, 0);
            eq_ctx.link(0, &mut sink, 0);
        } else {
            unsharp_ctx.link(0, &mut sink, 0);
        }

        graph
            .validate()
            .context("Failed to validate post-processing filter graph")?;

        info!(
            width,
            height,
            fps = format!("{:.1}", fps),
            pp = %pp_args,
            hqdn3d = %hqdn3d_args,
            unsharp = %unsharp_args,
            eq = %eq_args,
            "Post-processing filter graph created"
        );

        Ok(Self {
            graph,
            source,
            sink,
            filtered_frame: ffmpeg::frame::Video::empty(),
        })
    }

    /// Push `input` into the filter graph and call `callback` for every
    /// output frame the sink makes available.
    ///
    /// Temporal filters (e.g. `hqdn3d`) buffer one frame internally before
    /// producing output, so this may call `callback` zero or more times per
    /// invocation.  The loop drains all ready frames so no output is lost.
    fn process_into<F>(&mut self, input: &ffmpeg::frame::Video, mut callback: F) -> Result<()>
    where
        F: FnMut(&mut ffmpeg::frame::Video) -> Result<()>,
    {
        self.source
            .source()
            .add(input)
            .context("Failed to push frame into filter graph")?;

        while let Ok(()) = self.sink.sink().frame(&mut self.filtered_frame) {
            callback(&mut self.filtered_frame)?;
        }
        Ok(())
    }

    /// Signal end-of-stream to the filter graph and drain all buffered frames,
    /// calling `callback` for each remaining frame.
    fn flush<F>(&mut self, mut callback: F) -> Result<()>
    where
        F: FnMut(&mut ffmpeg::frame::Video) -> Result<()>,
    {
        self.source
            .source()
            .flush()
            .context("Failed to flush filter graph source")?;

        while let Ok(()) = self.sink.sink().frame(&mut self.filtered_frame) {
            callback(&mut self.filtered_frame)?;
        }
        Ok(())
    }
}

/// Scale a decoded video frame into `output`, optionally cropping first.
///
/// When `crop` is `Some`, only the specified rectangle from `input` is extracted
/// and scaled to fill `output`. This uses `sws_scale` directly so we can pass
/// adjusted source data pointers for the horizontal crop offset.
fn scale_with_crop(
    scaler: &mut ffmpeg::software::scaling::context::Context,
    input: &ffmpeg::util::frame::video::Video,
    output: &mut ffmpeg::util::frame::video::Video,
    crop: Option<CropRect>,
) -> Result<()> {
    match crop {
        None => {
            scaler
                .run(input, output)
                .map_err(|_| anyhow::anyhow!("Failed to scale video frame"))?;
        }
        Some(c) => {
            // The scaler was created with source dimensions = crop size.
            // We call sws_scale directly with adjusted source pointers so it
            // reads starting from (crop.x, crop.y) in the original frame.
            //
            // SAFETY: We operate on valid, non-null AVFrame pointers obtained from
            // ffmpeg-next's safe API. The source frame is guaranteed to be populated
            // by the decoder, and the destination is allocated below. Pointer offsets
            // stay within the frame's allocated planes because the crop rect is
            // clamped to video dimensions before export.
            let result = unsafe {
                if output.is_empty() {
                    output.alloc(
                        scaler.output().format,
                        scaler.output().width,
                        scaler.output().height,
                    );
                }

                let src_frame = input.as_ptr();
                let dst_frame = output.as_mut_ptr();

                let src_format = input.format();
                let src_linesizes = &(*src_frame).linesize;
                let src_data = &(*src_frame).data;

                // Calculate per-plane chroma subsampling factors.
                // For YUV420P: log2_chroma_w=1, log2_chroma_h=1 meaning chroma
                // planes are half width and half height.
                let desc = ffmpeg::ffi::av_pix_fmt_desc_get(src_format.into());
                let (log2_chroma_w, log2_chroma_h) = if !desc.is_null() {
                    ((*desc).log2_chroma_w as u32, (*desc).log2_chroma_h as u32)
                } else {
                    (0, 0)
                };

                let mut src_slices: [*const u8; 4] = [std::ptr::null(); 4];
                for i in 0..4 {
                    if src_data[i].is_null() {
                        continue;
                    }
                    let linesize = src_linesizes[i].abs();
                    // Chroma planes are subsampled: shift X/Y by the chroma factors.
                    let x_px = if i == 0 { c.x } else { c.x >> log2_chroma_w };
                    let y_px = if i == 0 { c.y } else { c.y >> log2_chroma_h };
                    src_slices[i] = src_data[i]
                        .wrapping_offset(y_px as isize * linesize as isize + x_px as isize);
                }

                ffmpeg::ffi::sws_scale(
                    scaler.as_mut_ptr(),
                    src_slices.as_ptr(),
                    src_linesizes.as_ptr() as *const _,
                    0,
                    c.height as c_int,
                    (*dst_frame).data.as_ptr(),
                    (*dst_frame).linesize.as_ptr() as *mut _,
                )
            };

            if result < 0 {
                return Err(anyhow::anyhow!(
                    "sws_scale failed with crop (error code {})",
                    result
                ));
            }
        }
    }
    Ok(())
}

fn seek_to_seconds(input_ctx: &mut ffmpeg::format::context::Input, position_secs: f64) {
    let ts = (position_secs.max(0.0) * f64::from(ffmpeg::ffi::AV_TIME_BASE)).round() as i64;
    let _ = input_ctx.seek(ts, ..);
}

fn time_base_to_secs_per_tick(time_base: ffmpeg::Rational) -> Option<f64> {
    let num = f64::from(time_base.numerator());
    let den = f64::from(time_base.denominator());
    if den <= 0.0 {
        return None;
    }
    Some(num / den)
}

/// Run stream copy export using ffmpeg-next (trim and concat without re-encoding).
pub fn run_stream_copy_export_sdk(
    request: &ClipExportRequest,
    progress_tx: &Sender<ClipExportUpdate>,
    cancel_flag: &Arc<AtomicBool>,
) -> Result<ExportOutcome> {
    let _ = progress_tx.send(ClipExportUpdate::Progress {
        phase: ClipExportPhase::Preparing,
        fraction: 0.0,
        message: "Stream copy export (no re-encoding)".to_string(),
    });

    info!(
        "Stream copy export {:?} -> {:?} ({} kept ranges)",
        request.input_path,
        request.output_path,
        request.keep_ranges.len()
    );

    let total_duration_secs = request.output_duration_secs().max(0.1);

    // Open input
    let mut input_ctx = ffmpeg::format::input(&request.input_path).with_context(|| {
        format!(
            "Failed to open input for stream copy: {:?}",
            request.input_path
        )
    })?;

    // Create output
    let mut output_ctx = ffmpeg::format::output(&request.output_path).with_context(|| {
        format!(
            "Failed to create output for stream copy: {:?}",
            request.output_path
        )
    })?;

    // Map streams (copy codec)
    let sanitized_fps_i32 = super::video_file::normalize_output_fps(
        request.output_fps.unwrap_or(request.metadata.fps),
        request.metadata.fps,
    )
    .round() as i32;

    let mut stream_mapping: Vec<(usize, usize, bool)> = vec![];
    for (stream_index, stream) in input_ctx.streams().enumerate() {
        let codec_params = stream.parameters();
        let medium = codec_params.medium();

        match medium {
            ffmpeg::media::Type::Video => {
                let mut out_stream = output_ctx
                    .add_stream(ffmpeg::encoder::find(ffmpeg::codec::Id::None))
                    .context("Failed to add video stream")?;
                // SAFETY: Clear codec_tag to prevent FFmpeg from attempting to convert
                // between container-specific codec tags during stream copy. The parameters
                // are immediately overwritten by set_parameters() below with valid input
                // codec parameters. This is a standard pattern for stream copy operations.
                unsafe {
                    (*out_stream.parameters().as_mut_ptr()).codec_tag = 0;
                }
                out_stream.set_parameters(codec_params);
                out_stream.set_time_base(stream.time_base());
                if sanitized_fps_i32 > 0 {
                    out_stream.set_avg_frame_rate((sanitized_fps_i32, 1));
                }
                stream_mapping.push((stream_index, out_stream.index(), true));
            }
            ffmpeg::media::Type::Audio if request.metadata.has_audio => {
                let mut out_stream = output_ctx
                    .add_stream(ffmpeg::encoder::find(ffmpeg::codec::Id::None))
                    .context("Failed to add audio stream")?;
                // SAFETY: Clear codec_tag to prevent FFmpeg from attempting to convert
                // between container-specific codec tags during stream copy. The parameters
                // are immediately overwritten by set_parameters() below with valid input
                // codec parameters. This is a standard pattern for stream copy operations.
                unsafe {
                    (*out_stream.parameters().as_mut_ptr()).codec_tag = 0;
                }
                out_stream.set_parameters(codec_params);
                out_stream.set_time_base(stream.time_base());
                stream_mapping.push((stream_index, out_stream.index(), true));
            }
            _ => {
                stream_mapping.push((stream_index, usize::MAX, false));
            }
        }
    }

    // Collect time bases for each stream before we start the packet iterator
    let time_bases: std::collections::HashMap<usize, ffmpeg::Rational> = input_ctx
        .streams()
        .map(|s| (s.index(), s.time_base()))
        .collect();

    // Set faststart
    let mut opts = ffmpeg::Dictionary::new();
    opts.set("movflags", "+faststart");
    output_ctx
        .write_header_with(opts)
        .with_context(|| "Failed to write MP4 header for stream copy")?;

    // Build stream lookup once so packet processing doesn't repeatedly scan stream mappings.
    #[derive(Clone, Copy)]
    struct StreamCopyRoute {
        out_idx: usize,
        in_time_base: ffmpeg::Rational,
        out_time_base: ffmpeg::Rational,
        is_video: bool,
    }
    let mut stream_routes: HashMap<usize, StreamCopyRoute> = HashMap::with_capacity(4);
    for (in_idx, out_idx, should_copy) in &stream_mapping {
        if !*should_copy || *out_idx == usize::MAX {
            continue;
        }
        let in_time_base = *time_bases
            .get(in_idx)
            .with_context(|| format!("Missing input time base for stream {}", in_idx))?;
        let out_time_base = output_ctx
            .stream(*out_idx)
            .with_context(|| format!("Missing output stream {}", out_idx))?
            .time_base();
        let is_video = input_ctx
            .stream(*in_idx)
            .map(|s| s.parameters().medium() == ffmpeg::media::Type::Video)
            .unwrap_or(false);
        stream_routes.insert(
            *in_idx,
            StreamCopyRoute {
                out_idx: *out_idx,
                in_time_base,
                out_time_base,
                is_video,
            },
        );
    }

    // Read packets by output range (seek per range), so stream-copy work scales with kept duration.
    let start_time = Instant::now();
    let mut last_progress_time = start_time;
    let mut processed_duration: f64 = 0.0;
    let mut output_cursor_secs = 0.0f64;

    let mut last_out_dts_by_stream: HashMap<usize, i64> = HashMap::with_capacity(4);
    let mut last_out_pts_by_stream: HashMap<usize, i64> = HashMap::with_capacity(4);
    for (range_index, range) in request.keep_ranges.iter().enumerate() {
        if cancel_flag.load(Ordering::Relaxed) {
            return Ok(ExportOutcome::Cancelled);
        }
        let range_started_at = Instant::now();
        let range_output_start_secs = output_cursor_secs;
        output_cursor_secs += range.duration_secs();
        seek_to_seconds(&mut input_ctx, range.start_secs);
        let mut streams_past_range_end: HashSet<usize> = HashSet::new();

        // Scan forward until we hit a keyframe at/after the requested range start
        // on each video stream. We must not "unlock" on a keyframe that is before
        // range.start_secs and then drop pre-start packets, because that leaves the
        // first in-range frames without their reference chain and can produce
        // white/corrupt leading frames in stream-copy output.
        let mut pending_range_start_keyframes: HashSet<usize> = stream_routes
            .iter()
            .filter(|(_, r)| r.is_video)
            .map(|(&idx, _)| idx)
            .collect();
        let mut keyframe_scan_done = pending_range_start_keyframes.is_empty();

        for (stream, mut packet) in input_ctx.packets() {
            if cancel_flag.load(Ordering::Relaxed) {
                return Ok(ExportOutcome::Cancelled);
            }
            let stream_index = stream.index();
            let Some(route) = stream_routes.get(&stream_index).copied() else {
                continue;
            };

            let secs_per_in_tick = match time_base_to_secs_per_tick(route.in_time_base) {
                Some(v) => v,
                None => continue,
            };
            let secs_per_out_tick = match time_base_to_secs_per_tick(route.out_time_base) {
                Some(v) => v,
                None => continue,
            };

            let pts_in = match packet.pts().or(packet.dts()) {
                Some(ts) => ts,
                None => continue,
            };
            let pts_secs = pts_in as f64 * secs_per_in_tick;

            // While finding start keyframes, drop packets from all streams.
            // This keeps A/V aligned to the actual video decode start point.
            if !keyframe_scan_done {
                if route.is_video && packet.is_key() && pts_secs >= range.start_secs {
                    pending_range_start_keyframes.remove(&stream_index);
                    if pending_range_start_keyframes.is_empty() {
                        keyframe_scan_done = true;
                    }
                }
                if !keyframe_scan_done {
                    continue;
                }
            }

            // Once all copied streams are comfortably past the range end, stop this range.
            if pts_secs >= range.end_secs + 0.75 {
                streams_past_range_end.insert(stream_index);
                if streams_past_range_end.len() >= stream_routes.len() {
                    break;
                }
                continue;
            }
            // Skip packets before the requested start (they were captured to satisfy
            // keyframe references but are outside the user's trim window).
            if pts_secs < range.start_secs || pts_secs >= range.end_secs {
                continue;
            }

            let output_pts_secs = range_output_start_secs + (pts_secs - range.start_secs);
            let adjusted_pts = (output_pts_secs / secs_per_out_tick).round() as i64;
            let pts_offset_secs = output_pts_secs - pts_secs;
            let adjusted_dts = packet.dts().or(packet.pts()).map(|dts_in| {
                let dts_secs = dts_in as f64 * secs_per_in_tick;
                let output_dts_secs = dts_secs + pts_offset_secs;
                (output_dts_secs / secs_per_out_tick).round() as i64
            });

            // Rescale timestamps to the output timebase before applying final PTS/DTS.
            packet.rescale_ts(route.in_time_base, route.out_time_base);
            packet.set_position(-1);
            packet.set_stream(route.out_idx);

            let mut fixed_pts = adjusted_pts.max(0);
            let mut fixed_dts = adjusted_dts.unwrap_or(fixed_pts).max(0);
            if fixed_pts < fixed_dts {
                fixed_pts = fixed_dts;
            }

            if let Some(last) = last_out_dts_by_stream.get(&route.out_idx) {
                if fixed_dts < *last {
                    fixed_dts = *last;
                }
            }
            if let Some(last) = last_out_pts_by_stream.get(&route.out_idx) {
                if fixed_pts < *last {
                    fixed_pts = *last;
                }
            }

            packet.set_dts(Some(fixed_dts));
            packet.set_pts(Some(fixed_pts));

            last_out_dts_by_stream.insert(route.out_idx, fixed_dts);
            last_out_pts_by_stream.insert(route.out_idx, fixed_pts);

            processed_duration = processed_duration.max(output_pts_secs);

            packet
                .write_interleaved(&mut output_ctx)
                .with_context(|| "Failed writing packet during stream copy")?;

            // Progress reporting - use structured logging to avoid format! allocations in hot path
            let now = Instant::now();
            const MIN_PROGRESS_INTERVAL: std::time::Duration =
                std::time::Duration::from_millis(100);
            if now.duration_since(last_progress_time) >= MIN_PROGRESS_INTERVAL {
                last_progress_time = now;
                let progress = (processed_duration / total_duration_secs).min(1.0) as f32;
                tracing::debug!(
                    processed_duration_secs = processed_duration,
                    progress_pct = progress * 100.0,
                    "Stream copy progress"
                );
                let _ = progress_tx.send(ClipExportUpdate::Progress {
                    phase: ClipExportPhase::Preparing,
                    fraction: progress,
                    message: format!("Stream copy: {:.1}s processed", processed_duration),
                });
            }
        }

        info!(
            range_index = range_index + 1,
            range_count = request.keep_ranges.len(),
            range_elapsed_secs = range_started_at.elapsed().as_secs_f64(),
            range_duration_secs = range.duration_secs(),
            "Stream copy range completed"
        );
    }

    output_ctx
        .write_trailer()
        .with_context(|| "Failed to finalize stream copy output")?;

    info!(
        elapsed_secs = start_time.elapsed().as_secs_f64(),
        output = ?request.output_path,
        "Stream copy export complete"
    );
    Ok(ExportOutcome::Finished(request.output_path.clone()))
}

/// Run export attempt using ffmpeg-next filter graphs.
#[allow(clippy::too_many_arguments)]
pub(crate) fn attempt_export(
    request: &ClipExportRequest,
    output_path: &Path,
    video_bitrate_kbps: u32,
    audio_bitrate_kbps: u32,
    video_encoder: ExportVideoEncoder,
    progress_tx: &Sender<ClipExportUpdate>,
    cancel_flag: &Arc<AtomicBool>,
    attempt_index: usize,
    attempt_count: usize,
    single_pass_phase: ClipExportPhase,
) -> Result<Option<ExportAttemptResult>> {
    let total_duration_secs = request.output_duration_secs().max(0.1);

    let _ = progress_tx.send(ClipExportUpdate::Progress {
        phase: ClipExportPhase::Preparing,
        fraction: 0.0,
        message: "Preparing export".to_string(),
    });

    info!(
        "Exporting clipped video {:?} -> {:?} ({} kept ranges, target={} MB, video bitrate={} kbps, encoder={})",
        request.input_path,
        output_path,
        request.keep_ranges.len(),
        request.target_size_mb,
        video_bitrate_kbps,
        video_encoder.ffmpeg_name()
    );

    // Open input
    let mut input_ctx = ffmpeg::format::input(&request.input_path)
        .with_context(|| format!("Failed to open input: {:?}", request.input_path))?;

    // Find video and audio streams
    let video_stream_idx = input_ctx
        .streams()
        .best(ffmpeg::media::Type::Video)
        .map(|s| s.index())
        .context("No video stream found")?;

    let audio_stream_idx = if request.metadata.has_audio {
        input_ctx
            .streams()
            .best(ffmpeg::media::Type::Audio)
            .map(|s| s.index())
    } else {
        None
    };

    let video_stream = input_ctx
        .stream(video_stream_idx)
        .context("Missing video stream")?;
    let input_time_base = video_stream.time_base();

    // Store audio time base separately (Bug 4 fix: audio has different time base than video)
    let audio_input_time_base = if let Some(audio_idx) = audio_stream_idx {
        input_ctx.stream(audio_idx).map(|s| s.time_base())
    } else {
        None
    };

    // Create output
    let mut output_ctx = ffmpeg::format::output(output_path)
        .with_context(|| format!("Failed to create output: {:?}", output_path))?;

    // Add video stream with encoder - use encoder name to get hardware encoder if needed
    let codec_name = video_encoder.ffmpeg_name();
    let codec = ffmpeg::encoder::find_by_name(codec_name)
        .with_context(|| format!("Encoder {} not found", codec_name))?;

    let output_width = request.output_width.unwrap_or(request.metadata.width);
    let output_height = request.output_height.unwrap_or(request.metadata.height);
    let crop = request.crop;

    let output_fps_i32 = super::video_file::normalize_output_fps(
        request.output_fps.unwrap_or(request.metadata.fps),
        request.metadata.fps,
    )
    .round() as i32;

    // Create encoder context and configure it
    let encoder_ctx = ffmpeg::codec::context::Context::new_with_codec(codec);
    let mut ffmpeg_video_enc = encoder_ctx
        .encoder()
        .video()
        .context("Failed to create video encoder")?;
    ffmpeg_video_enc.set_width(output_width);
    ffmpeg_video_enc.set_height(output_height);
    ffmpeg_video_enc.set_time_base(ffmpeg::Rational(1, output_fps_i32));
    ffmpeg_video_enc.set_frame_rate(Some((output_fps_i32, 1)));
    ffmpeg_video_enc.set_bit_rate((video_bitrate_kbps * 1000) as usize);
    ffmpeg_video_enc.set_max_bit_rate((video_bitrate_kbps * 1000) as usize);
    ffmpeg_video_enc.set_format(ffmpeg::format::Pixel::YUV420P);
    ffmpeg_video_enc.set_max_b_frames(0);

    // Set codec-specific options
    let mut opts = ffmpeg::Dictionary::new();
    match video_encoder {
        ExportVideoEncoder::SoftwareHevc => {
            opts.set("preset", "slow");
        }
        ExportVideoEncoder::HevcNvenc => {
            opts.set("preset", "p7");
            opts.set("tune", "hq");
            opts.set("rc", "vbr");
        }
        ExportVideoEncoder::HevcAmf => {
            opts.set("quality", "quality");
            opts.set("rc", "vbr_peak");
        }
        ExportVideoEncoder::HevcQsv => {
            opts.set("preset", "medium");
            opts.set("look_ahead", "1");
        }
    }

    // Open the encoder - this consumes the ffmpeg_video_enc and returns an Encoder
    let mut opened_video_encoder = ffmpeg_video_enc
        .open_with(opts)
        .context("Failed to open encoder")?;

    // Now create the output stream using the opened encoder
    let video_out_idx = {
        let mut video_out_stream = output_ctx
            .add_stream(codec)
            .context("Failed to add video stream")?;
        video_out_stream.set_time_base(ffmpeg::Rational(1, output_fps_i32));
        video_out_stream.set_avg_frame_rate((output_fps_i32, 1));
        video_out_stream.set_parameters(&opened_video_encoder);
        video_out_stream.index()
    };

    // Add audio stream if present
    let mut opened_audio_encoder = None;
    let mut audio_out_time_base = None;
    let audio_out_idx = if audio_stream_idx.is_some() {
        let audio_codec =
            ffmpeg::encoder::find(ffmpeg::codec::Id::AAC).context("AAC encoder not found")?;

        let audio_enc_ctx = ffmpeg::codec::context::Context::new_with_codec(audio_codec);
        let mut audio_encoder = audio_enc_ctx
            .encoder()
            .audio()
            .context("Failed to create audio encoder")?;
        audio_encoder.set_time_base((1, 48_000));
        audio_encoder.set_bit_rate((audio_bitrate_kbps.max(48) * 1000) as usize);
        audio_encoder.set_rate(48000);
        audio_encoder.set_channel_layout(ffmpeg::channel_layout::ChannelLayout::STEREO);
        audio_encoder.set_format(ffmpeg::format::Sample::F32(
            ffmpeg::format::sample::Type::Planar,
        ));
        let opened = audio_encoder
            .open()
            .context("Failed to open audio encoder")?;

        let (audio_idx_out, audio_tb_out) = {
            let mut audio_out_stream = output_ctx
                .add_stream(ffmpeg::encoder::find(ffmpeg::codec::Id::AAC))
                .context("Failed to add audio stream")?;
            audio_out_stream.set_time_base((1, 48_000));
            audio_out_stream.set_parameters(&opened);
            (audio_out_stream.index(), audio_out_stream.time_base())
        };

        opened_audio_encoder = Some(opened);
        audio_out_time_base = Some(audio_tb_out);
        Some(audio_idx_out)
    } else {
        None
    };

    // Write header
    let mut opts = ffmpeg::Dictionary::new();
    opts.set("movflags", "+faststart");
    output_ctx
        .write_header_with(opts)
        .with_context(|| "Failed to write output header")?;

    // The muxer may adjust stream time bases after the header is written. Re-read them now;
    // otherwise rescaling packets with a stale time base can corrupt timestamps / implied FPS.
    let video_out_time_base = output_ctx
        .stream(video_out_idx)
        .context("Missing output video stream")?
        .time_base();
    if let Some(audio_idx) = audio_out_idx {
        audio_out_time_base = Some(
            output_ctx
                .stream(audio_idx)
                .context("Missing output audio stream")?
                .time_base(),
        );
    }

    // We'll decode only the kept ranges by seeking for each range, instead of decoding the
    // entire input file and filtering frames. This makes export time proportional to the
    // kept duration rather than the source clip duration.

    // Use normalized FPS to avoid issues with corrupted metadata (e.g., 10k+ FPS)
    let output_fps = super::video_file::normalize_output_fps(
        request.output_fps.unwrap_or(request.metadata.fps),
        request.metadata.fps,
    );

    let video_enc_time_base = opened_video_encoder.time_base();
    let video_enc_secs_per_tick = time_base_to_secs_per_tick(video_enc_time_base)
        .unwrap_or_else(|| 1.0 / output_fps.max(1.0));
    let video_enc_ticks_per_frame = ((1.0 / output_fps) / video_enc_secs_per_tick)
        .round()
        .max(1.0) as i64;

    let video_ticks_per_second = f64::from(video_out_time_base.denominator())
        / f64::from(video_out_time_base.numerator().max(1));
    let video_default_duration = (video_ticks_per_second / output_fps).round().max(1.0) as i64;
    let audio_default_duration = if let Some(audio_tb) = audio_out_time_base {
        let audio_ticks_per_second =
            f64::from(audio_tb.denominator()) / f64::from(audio_tb.numerator().max(1));
        ((AAC_FRAME_SAMPLES as f64) * (audio_ticks_per_second / 48_000.0))
            .round()
            .max(1.0) as i64
    } else {
        AAC_FRAME_SAMPLES
    };

    let mut next_video_pts = 0i64;
    let mut next_video_dts = 0i64;
    let mut next_audio_pts = 0i64;
    let mut next_audio_dts = 0i64;
    let mut next_video_frame_pts = 0i64;

    let start_time = Instant::now();
    let mut last_progress_time = start_time;
    let mut processed_duration: f64 = 0.0;

    // Process kept ranges by seeking.
    let mut decoder_setup_elapsed_secs = 0.0f64;
    let mut scale_elapsed_secs = 0.0f64;
    let mut video_encode_elapsed_secs = 0.0f64;
    let mut audio_encode_elapsed_secs = 0.0f64;
    let mut seek_elapsed_secs = 0.0f64;
    let mut range_init_overhead_elapsed_secs = 0.0f64;
    let mut range_process_elapsed_secs = 0.0f64;
    // Reuse frame allocations across ranges to reduce per-range allocation churn.
    let mut decoded_video = ffmpeg::util::frame::video::Video::empty();
    let mut scaled_video = ffmpeg::util::frame::video::Video::new(
        ffmpeg::format::Pixel::YUV420P,
        output_width,
        output_height,
    );
    let mut decoded_audio = ffmpeg::util::frame::audio::Audio::empty();
    let mut output_cursor_secs = 0.0f64;

    for range in &request.keep_ranges {
        let range_started_at = Instant::now();
        let range_output_start_secs = output_cursor_secs;
        output_cursor_secs += range.duration_secs();

        if cancel_flag.load(Ordering::Relaxed) {
            return Ok(None);
        }

        let seek_started_at = Instant::now();
        seek_to_seconds(&mut input_ctx, range.start_secs);
        seek_elapsed_secs += seek_started_at.elapsed().as_secs_f64();

        // Recreate decoders after each seek.
        let decoder_setup_started_at = Instant::now();
        let v_stream = input_ctx
            .stream(video_stream_idx)
            .with_context(|| format!("missing video stream {} after seek", video_stream_idx))?;
        let v_ctx = ffmpeg::codec::context::Context::from_parameters(v_stream.parameters())?;
        let mut video_decoder = v_ctx.decoder().video()?;
        // When crop is active, source is the crop rectangle. Otherwise, source is the decoder's
        // actual frame dimensions (not the output dimensions - the scaler needs to know the input
        // frame size to properly scale from input to output).
        let (src_w, src_h) = crop.map_or((video_decoder.width(), video_decoder.height()), |c| {
            (c.width, c.height)
        });
        let mut video_scaler = ffmpeg::software::scaling::Context::get(
            video_decoder.format(),
            src_w,
            src_h,
            ffmpeg::format::Pixel::YUV420P,
            output_width,
            output_height,
            ffmpeg::software::scaling::flag::Flags::BILINEAR,
        )
        .context("Failed to create video scaler for export")?;

        // Build a fresh post-processing filter graph for each range.
        // The graph cannot be reused across ranges because flushing signals EOF
        // to the buffer source, after which no more frames can be pushed.
        let mut post_process_graph: Option<PostProcessFilterGraph> = None;
        if request.post_process_filters {
            let strengths = compute_filter_strengths(
                video_bitrate_kbps,
                output_width,
                output_height,
                output_fps,
            );
            match PostProcessFilterGraph::new(output_width, output_height, output_fps, strengths) {
                Ok(graph) => {
                    info!(
                        unsharp_luma = strengths.unsharp_luma,
                        unsharp_size =
                            format!("{}x{}", strengths.unsharp_size_x, strengths.unsharp_size_y),
                        hqdn3d_luma_spatial = strengths.hqdn3d_luma_spatial,
                        hqdn3d_luma_temporal = strengths.hqdn3d_luma_temporal,
                        pp_enable = strengths.pp_enable,
                        eq_contrast = format!("{:.3}", strengths.eq_contrast),
                        bpp = format!(
                            "{:.4}",
                            (f64::from(video_bitrate_kbps) * 1000.0)
                                / (f64::from(output_width)
                                    * f64::from(output_height)
                                    * output_fps.max(1.0))
                        ),
                        "Post-processing filter graph created for range"
                    );
                    post_process_graph = Some(graph);
                }
                Err(err) => {
                    warn!("Failed to create post-processing filter graph, disabling: {err:#}");
                }
            }
        }

        let mut audio_decoder = None;
        let mut audio_resampler = None;
        if let Some(audio_idx) = audio_stream_idx {
            let a_stream = input_ctx
                .stream(audio_idx)
                .with_context(|| format!("missing audio stream {} after seek", audio_idx))?;
            let a_ctx = ffmpeg::codec::context::Context::from_parameters(a_stream.parameters())?;
            let decoder = a_ctx.decoder().audio()?;

            if let Some(ref encoder) = opened_audio_encoder {
                let resampler = ffmpeg::software::resampling::Context::get(
                    decoder.format(),
                    decoder.channel_layout(),
                    decoder.rate(),
                    encoder.format(),
                    encoder.channel_layout(),
                    encoder.rate(),
                )?;
                audio_resampler = Some(resampler);
            }
            audio_decoder = Some(decoder);
        }
        decoder_setup_elapsed_secs += decoder_setup_started_at.elapsed().as_secs_f64();

        range_init_overhead_elapsed_secs += range_started_at.elapsed().as_secs_f64();

        let mut stop_video = false;
        let mut stop_audio = audio_stream_idx.is_none();

        // Track whether we've found the first keyframe for this range.
        // After seeking, FFmpeg may land on a P/B frame whose reference frames
        // are outside the kept range. We must skip frames until we hit a keyframe.
        let mut found_first_keyframe = false;
        // Track if the first frame of this range has been encoded (to force a keyframe).
        let mut first_frame_encoded = false;

        // Read packets until we're safely beyond the range end, then flush decoders.
        for (stream, packet) in input_ctx.packets() {
            if cancel_flag.load(Ordering::Relaxed) {
                return Ok(None);
            }

            let stream_idx = stream.index();
            if stream_idx == video_stream_idx {
                if let Some(pts) = packet.pts() {
                    let pts_secs = pts as f64 * f64::from(input_time_base.numerator())
                        / f64::from(input_time_base.denominator());
                    if pts_secs > range.end_secs + 1.0 {
                        stop_video = true;
                    }
                }

                // Skip packets until we find a keyframe (for decodable output).
                // This mirrors the keyframe scanning in stream-copy export.
                if !found_first_keyframe {
                    let is_keyframe = packet.is_key();
                    if !is_keyframe {
                        continue;
                    }
                    found_first_keyframe = true;
                }

                video_decoder.send_packet(&packet)?;
                while video_decoder.receive_frame(&mut decoded_video).is_ok() {
                    let pts_secs = decoded_video
                        .timestamp()
                        .map(|ts| {
                            ts as f64 * f64::from(input_time_base.numerator())
                                / f64::from(input_time_base.denominator())
                        })
                        .unwrap_or(0.0);

                    if pts_secs >= range.end_secs {
                        continue;
                    }
                    if pts_secs < range.start_secs {
                        continue;
                    }

                    let output_pts_secs = range_output_start_secs + (pts_secs - range.start_secs);

                    let next_frame_time_secs =
                        (next_video_frame_pts as f64) * video_enc_secs_per_tick;
                    if output_pts_secs + 0.000_5 < next_frame_time_secs {
                        continue;
                    }

                    let scale_started_at = Instant::now();
                    scale_with_crop(&mut video_scaler, &decoded_video, &mut scaled_video, crop)
                        .context("Failed to scale video frame during export")?;
                    scale_elapsed_secs += scale_started_at.elapsed().as_secs_f64();
                    scaled_video.set_pts(Some(next_video_frame_pts));
                    next_video_frame_pts =
                        next_video_frame_pts.saturating_add(video_enc_ticks_per_frame.max(1));

                    // Force a keyframe at the start of each range to ensure
                    // the output is decodable when concatenating segments.
                    // This prevents artifacts at segment boundaries.
                    if !first_frame_encoded {
                        unsafe {
                            (*scaled_video.as_mut_ptr()).pict_type =
                                ffmpeg::picture::Type::I.into();
                            (*scaled_video.as_mut_ptr()).key_frame = 1;
                        }
                        first_frame_encoded = true;
                    }

                    let video_encode_started_at = Instant::now();

                    // Apply post-processing filters if enabled, otherwise encode
                    // the scaled frame directly.  process_into() drains all frames
                    // the filter makes available (temporal filters may output 0 or
                    // more frames per push), so no frames are ever silently lost.
                    if let Some(ref mut filter_graph) = post_process_graph {
                        filter_graph.process_into(&scaled_video, |filtered| {
                            opened_video_encoder.send_frame(filtered)?;
                            Ok(())
                        })?;
                    } else {
                        opened_video_encoder.send_frame(&scaled_video)?;
                    }

                    let mut pkt = ffmpeg::Packet::empty();
                    while opened_video_encoder.receive_packet(&mut pkt).is_ok() {
                        pkt.rescale_ts(opened_video_encoder.time_base(), video_out_time_base);
                        if pkt.pts().is_none() {
                            pkt.set_pts(Some(next_video_pts));
                        }
                        if pkt.dts().is_none() {
                            pkt.set_dts(Some(next_video_dts));
                        }
                        pkt.set_duration(video_default_duration);
                        let fixed_dts = pkt.dts().unwrap_or(next_video_dts).max(next_video_dts);
                        let fixed_pts = pkt.pts().unwrap_or(fixed_dts).max(fixed_dts);
                        pkt.set_dts(Some(fixed_dts));
                        pkt.set_pts(Some(fixed_pts));
                        pkt.set_stream(video_out_idx);
                        pkt.write_interleaved(&mut output_ctx)?;
                        next_video_dts = fixed_dts.saturating_add(pkt.duration().max(1));
                        next_video_pts = fixed_pts.saturating_add(pkt.duration().max(1));
                    }
                    video_encode_elapsed_secs += video_encode_started_at.elapsed().as_secs_f64();

                    processed_duration = processed_duration.max(output_pts_secs);
                }
            } else if Some(stream_idx) == audio_stream_idx {
                if let Some(pts) = packet.pts() {
                    let audio_tb = audio_input_time_base.unwrap_or(input_time_base);
                    let pts_secs = pts as f64 * f64::from(audio_tb.numerator())
                        / f64::from(audio_tb.denominator());
                    if pts_secs > range.end_secs + 0.5 {
                        stop_audio = true;
                    }
                }

                if let Some(ref mut decoder) = audio_decoder {
                    decoder.send_packet(&packet)?;
                    while decoder.receive_frame(&mut decoded_audio).is_ok() {
                        let audio_tb = audio_input_time_base.unwrap_or(input_time_base);
                        let pts_secs = decoded_audio
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

                        let output_pts_secs =
                            range_output_start_secs + (pts_secs - range.start_secs);

                        if let Some(ref encoder) = opened_audio_encoder {
                            let output_time_base = encoder.time_base();
                            let output_pts = (output_pts_secs
                                * f64::from(output_time_base.denominator())
                                / f64::from(output_time_base.numerator()))
                                as i64;
                            decoded_audio.set_pts(Some(output_pts));
                        }

                        if let Some(ref mut encoder) = opened_audio_encoder {
                            let audio_encode_started_at = Instant::now();
                            if let Some(ref mut resampler) = audio_resampler {
                                let mut resampled = ffmpeg::util::frame::audio::Audio::empty();
                                resampler.run(&decoded_audio, &mut resampled)?;
                                resampled.set_pts(decoded_audio.pts());
                                encoder.send_frame(&resampled)?;
                            } else {
                                encoder.send_frame(&decoded_audio)?;
                            }
                            let mut audio_pkt = ffmpeg::Packet::empty();
                            while encoder.receive_packet(&mut audio_pkt).is_ok() {
                                if let (Some(audio_idx), Some(audio_tb)) =
                                    (audio_out_idx, audio_out_time_base)
                                {
                                    audio_pkt.rescale_ts(encoder.time_base(), audio_tb);
                                    if audio_pkt.pts().is_none() {
                                        audio_pkt.set_pts(Some(next_audio_pts));
                                    }
                                    if audio_pkt.dts().is_none() {
                                        audio_pkt.set_dts(Some(next_audio_dts));
                                    }
                                    if audio_pkt.duration() <= 0
                                        || audio_pkt.duration() == INVALID_DURATION
                                    {
                                        audio_pkt.set_duration(audio_default_duration);
                                    }
                                    let fixed_dts = audio_pkt
                                        .dts()
                                        .unwrap_or(next_audio_dts)
                                        .max(next_audio_dts);
                                    let fixed_pts =
                                        audio_pkt.pts().unwrap_or(fixed_dts).max(fixed_dts);
                                    audio_pkt.set_dts(Some(fixed_dts));
                                    audio_pkt.set_pts(Some(fixed_pts));
                                    audio_pkt.set_stream(audio_idx);
                                    audio_pkt.write_interleaved(&mut output_ctx)?;
                                    next_audio_dts =
                                        fixed_dts.saturating_add(audio_pkt.duration().max(1));
                                    next_audio_pts =
                                        fixed_pts.saturating_add(audio_pkt.duration().max(1));
                                }
                            }
                            audio_encode_elapsed_secs +=
                                audio_encode_started_at.elapsed().as_secs_f64();
                        }
                    }
                }
            }

            if stop_video && stop_audio {
                break;
            }

            let now = Instant::now();
            const MIN_PROGRESS_INTERVAL: std::time::Duration =
                std::time::Duration::from_millis(100);
            if now.duration_since(last_progress_time) >= MIN_PROGRESS_INTERVAL {
                last_progress_time = now;
                let progress = (processed_duration / total_duration_secs).min(1.0) as f32;
                let _ = progress_tx.send(ClipExportUpdate::Progress {
                    phase: if video_encoder.supports_two_pass()
                        && single_pass_phase == ClipExportPhase::FirstPass
                    {
                        ClipExportPhase::FirstPass
                    } else {
                        single_pass_phase
                    },
                    fraction: progress,
                    message: format!(
                        "Attempt {}/{} - {:.1}s processed",
                        attempt_index + 1,
                        attempt_count,
                        processed_duration
                    ),
                });
            }
        }

        // Flush decoders for this range.
        video_decoder.send_eof()?;
        while video_decoder.receive_frame(&mut decoded_video).is_ok() {
            let pts_secs = decoded_video
                .timestamp()
                .map(|ts| {
                    ts as f64 * f64::from(input_time_base.numerator())
                        / f64::from(input_time_base.denominator())
                })
                .unwrap_or(0.0);
            if !(pts_secs >= range.start_secs && pts_secs < range.end_secs) {
                continue;
            }
            let output_pts_secs = range_output_start_secs + (pts_secs - range.start_secs);

            let next_frame_time_secs = (next_video_frame_pts as f64) * video_enc_secs_per_tick;
            if output_pts_secs + 0.000_5 < next_frame_time_secs {
                continue;
            }
            let scale_started_at = Instant::now();
            scale_with_crop(&mut video_scaler, &decoded_video, &mut scaled_video, crop)
                .context("Failed to scale video frame during export")?;
            scale_elapsed_secs += scale_started_at.elapsed().as_secs_f64();
            scaled_video.set_pts(Some(next_video_frame_pts));
            next_video_frame_pts =
                next_video_frame_pts.saturating_add(video_enc_ticks_per_frame.max(1));
            let video_encode_started_at = Instant::now();
            if let Some(ref mut filter_graph) = post_process_graph {
                filter_graph
                    .process_into(&scaled_video, |filtered| {
                        opened_video_encoder.send_frame(filtered)?;
                        Ok(())
                    })
                    .context("Post-processing filter failed during flush")?;
            } else {
                opened_video_encoder.send_frame(&scaled_video)?;
            }
            let mut pkt = ffmpeg::Packet::empty();
            while opened_video_encoder.receive_packet(&mut pkt).is_ok() {
                pkt.rescale_ts(opened_video_encoder.time_base(), video_out_time_base);
                if pkt.pts().is_none() {
                    pkt.set_pts(Some(next_video_pts));
                }
                if pkt.dts().is_none() {
                    pkt.set_dts(Some(next_video_dts));
                }
                pkt.set_duration(video_default_duration);
                let fixed_dts = pkt.dts().unwrap_or(next_video_dts).max(next_video_dts);
                let fixed_pts = pkt.pts().unwrap_or(fixed_dts).max(fixed_dts);
                pkt.set_dts(Some(fixed_dts));
                pkt.set_pts(Some(fixed_pts));
                pkt.set_stream(video_out_idx);
                pkt.write_interleaved(&mut output_ctx)?;
                next_video_dts = fixed_dts.saturating_add(pkt.duration().max(1));
                next_video_pts = fixed_pts.saturating_add(pkt.duration().max(1));
            }
            video_encode_elapsed_secs += video_encode_started_at.elapsed().as_secs_f64();
            processed_duration = processed_duration.max(output_pts_secs);
        }

        // Flush any buffered frames from the post-processing filter graph.
        // Temporal filters may hold frames back; flushing signals end-of-stream
        // so all buffered frames are released and encoded.
        if let Some(ref mut filter_graph) = post_process_graph {
            if let Err(err) = filter_graph.flush(|filtered| {
                filtered.set_pts(Some(next_video_frame_pts));
                next_video_frame_pts =
                    next_video_frame_pts.saturating_add(video_enc_ticks_per_frame.max(1));
                let video_encode_started_at = Instant::now();
                opened_video_encoder.send_frame(filtered)?;
                let mut pkt = ffmpeg::Packet::empty();
                while opened_video_encoder.receive_packet(&mut pkt).is_ok() {
                    pkt.rescale_ts(opened_video_encoder.time_base(), video_out_time_base);
                    if pkt.pts().is_none() {
                        pkt.set_pts(Some(next_video_pts));
                    }
                    if pkt.dts().is_none() {
                        pkt.set_dts(Some(next_video_dts));
                    }
                    if pkt.duration() <= 0 || pkt.duration() == INVALID_DURATION {
                        pkt.set_duration(video_default_duration);
                    }
                    let fixed_dts = pkt.dts().unwrap_or(next_video_dts).max(next_video_dts);
                    let fixed_pts = pkt.pts().unwrap_or(fixed_dts).max(fixed_dts);
                    pkt.set_dts(Some(fixed_dts));
                    pkt.set_pts(Some(fixed_pts));
                    pkt.set_stream(video_out_idx);
                    pkt.write_interleaved(&mut output_ctx)?;
                    next_video_dts = fixed_dts.saturating_add(pkt.duration().max(1));
                    next_video_pts = fixed_pts.saturating_add(pkt.duration().max(1));
                }
                video_encode_elapsed_secs += video_encode_started_at.elapsed().as_secs_f64();
                Ok(())
            }) {
                warn!("Failed to flush post-processing filter graph: {err:#}");
            }
        }

        if let Some(ref mut decoder) = audio_decoder {
            decoder.send_eof()?;
            while decoder.receive_frame(&mut decoded_audio).is_ok() {
                let audio_tb = audio_input_time_base.unwrap_or(input_time_base);
                let pts_secs = decoded_audio
                    .timestamp()
                    .map(|ts| {
                        ts as f64 * f64::from(audio_tb.numerator())
                            / f64::from(audio_tb.denominator())
                    })
                    .unwrap_or(0.0);
                if !(pts_secs >= range.start_secs && pts_secs < range.end_secs) {
                    continue;
                }
                let output_pts_secs = range_output_start_secs + (pts_secs - range.start_secs);
                if let Some(ref encoder) = opened_audio_encoder {
                    let output_time_base = encoder.time_base();
                    let output_pts = (output_pts_secs * f64::from(output_time_base.denominator())
                        / f64::from(output_time_base.numerator()))
                        as i64;
                    decoded_audio.set_pts(Some(output_pts));
                }
                if let Some(ref mut encoder) = opened_audio_encoder {
                    let audio_encode_started_at = Instant::now();
                    if let Some(ref mut resampler) = audio_resampler {
                        let mut resampled = ffmpeg::util::frame::audio::Audio::empty();
                        resampler.run(&decoded_audio, &mut resampled)?;
                        resampled.set_pts(decoded_audio.pts());
                        encoder.send_frame(&resampled)?;
                    } else {
                        encoder.send_frame(&decoded_audio)?;
                    }
                    let mut audio_pkt = ffmpeg::Packet::empty();
                    while encoder.receive_packet(&mut audio_pkt).is_ok() {
                        if let (Some(audio_idx), Some(audio_tb)) =
                            (audio_out_idx, audio_out_time_base)
                        {
                            audio_pkt.rescale_ts(encoder.time_base(), audio_tb);
                            if audio_pkt.pts().is_none() {
                                audio_pkt.set_pts(Some(next_audio_pts));
                            }
                            if audio_pkt.dts().is_none() {
                                audio_pkt.set_dts(Some(next_audio_dts));
                            }
                            if audio_pkt.duration() <= 0 || audio_pkt.duration() == INVALID_DURATION
                            {
                                audio_pkt.set_duration(audio_default_duration);
                            }
                            let fixed_dts = audio_pkt
                                .dts()
                                .unwrap_or(next_audio_dts)
                                .max(next_audio_dts);
                            let fixed_pts = audio_pkt.pts().unwrap_or(fixed_dts).max(fixed_dts);
                            audio_pkt.set_dts(Some(fixed_dts));
                            audio_pkt.set_pts(Some(fixed_pts));
                            audio_pkt.set_stream(audio_idx);
                            audio_pkt.write_interleaved(&mut output_ctx)?;
                            next_audio_dts = fixed_dts.saturating_add(audio_pkt.duration().max(1));
                            next_audio_pts = fixed_pts.saturating_add(audio_pkt.duration().max(1));
                        }
                    }
                    audio_encode_elapsed_secs += audio_encode_started_at.elapsed().as_secs_f64();
                }
            }
        }
        let range_elapsed_secs = range_started_at.elapsed().as_secs_f64();
        range_process_elapsed_secs += range_elapsed_secs;
        info!(
            range_start_secs = range.start_secs,
            range_end_secs = range.end_secs,
            range_duration_secs = range.duration_secs(),
            range_elapsed_secs,
            "Export range processing completed"
        );
    }

    // Flush audio encoder
    if let Some(ref mut encoder) = opened_audio_encoder {
        encoder.send_eof()?;
        let mut audio_pkt = ffmpeg::Packet::empty();
        while encoder.receive_packet(&mut audio_pkt).is_ok() {
            if let (Some(audio_idx), Some(audio_tb)) = (audio_out_idx, audio_out_time_base) {
                audio_pkt.rescale_ts(encoder.time_base(), audio_tb);
                if audio_pkt.pts().is_none() {
                    audio_pkt.set_pts(Some(next_audio_pts));
                }
                if audio_pkt.dts().is_none() {
                    audio_pkt.set_dts(Some(next_audio_dts));
                }
                if audio_pkt.duration() <= 0 || audio_pkt.duration() == INVALID_DURATION {
                    audio_pkt.set_duration(audio_default_duration);
                }
                let fixed_dts = audio_pkt
                    .dts()
                    .unwrap_or(next_audio_dts)
                    .max(next_audio_dts);
                let fixed_pts = audio_pkt.pts().unwrap_or(fixed_dts).max(fixed_dts);
                audio_pkt.set_dts(Some(fixed_dts));
                audio_pkt.set_pts(Some(fixed_pts));
                audio_pkt.set_stream(audio_idx);
                audio_pkt.write_interleaved(&mut output_ctx)?;
                next_audio_dts = fixed_dts.saturating_add(audio_pkt.duration().max(1));
                next_audio_pts = fixed_pts.saturating_add(audio_pkt.duration().max(1));
            }
        }
    }

    // Flush video encoder
    opened_video_encoder.send_eof()?;
    let mut pkt = ffmpeg::Packet::empty();
    while opened_video_encoder.receive_packet(&mut pkt).is_ok() {
        pkt.rescale_ts(opened_video_encoder.time_base(), video_out_time_base);
        if pkt.pts().is_none() {
            pkt.set_pts(Some(next_video_pts));
        }
        if pkt.dts().is_none() {
            pkt.set_dts(Some(next_video_dts));
        }
        pkt.set_duration(video_default_duration);
        let fixed_dts = pkt.dts().unwrap_or(next_video_dts).max(next_video_dts);
        let fixed_pts = pkt.pts().unwrap_or(fixed_dts).max(fixed_dts);
        pkt.set_dts(Some(fixed_dts));
        pkt.set_pts(Some(fixed_pts));
        pkt.set_stream(video_out_idx);
        pkt.write_interleaved(&mut output_ctx)?;
        next_video_dts = fixed_dts.saturating_add(pkt.duration().max(1));
        next_video_pts = fixed_pts.saturating_add(pkt.duration().max(1));
    }

    output_ctx
        .write_trailer()
        .with_context(|| "Failed to write output trailer")?;
    info!(
        elapsed_secs = start_time.elapsed().as_secs_f64(),
        seek_elapsed_secs,
        decoder_setup_elapsed_secs,
        range_init_overhead_elapsed_secs,
        range_process_elapsed_secs,
        scale_elapsed_secs,
        video_encode_elapsed_secs,
        audio_encode_elapsed_secs,
        "Export attempt stage timings"
    );

    let size_bytes = std::fs::metadata(output_path)
        .with_context(|| format!("Failed to get size of export output file {:?}", output_path))?
        .len();

    Ok(Some(ExportAttemptResult {
        output_path: output_path.to_path_buf(),
        video_bitrate_kbps,
        size_bytes,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn high_bpp_produces_minimal_filtering() {
        // 1080p60 at 50 Mbps → bpp ≈ 0.40
        let s = compute_filter_strengths(50_000, 1920, 1080, 60.0);
        assert!(
            s.unsharp_luma <= 0.5,
            "expected light sharpen at high bpp, got {}",
            s.unsharp_luma
        );
        assert_eq!(
            s.hqdn3d_luma_spatial, 0.0,
            "expected no denoising at high bpp"
        );
    }

    #[test]
    fn medium_bpp_produces_moderate_filtering() {
        // 1080p60 at 5000 kbps → bpp ≈ 0.040
        let s = compute_filter_strengths(5_000, 1920, 1080, 60.0);
        assert!(
            s.unsharp_luma >= 0.5 && s.unsharp_luma <= 1.0,
            "expected moderate sharpen at medium bpp, got {}",
            s.unsharp_luma
        );
        assert!(
            s.hqdn3d_luma_spatial > 0.0,
            "expected denoising at medium bpp"
        );
        assert!(
            s.hqdn3d_luma_spatial < 6.0,
            "expected bounded denoising at medium bpp"
        );
    }

    #[test]
    fn low_bpp_produces_aggressive_denoising() {
        // 1080p60 at 500 kbps → bpp ≈ 0.004
        let s = compute_filter_strengths(500, 1920, 1080, 60.0);
        assert!(
            s.hqdn3d_luma_spatial >= 4.0,
            "expected aggressive denoising at low bpp, got {}",
            s.hqdn3d_luma_spatial
        );
        assert!(
            s.unsharp_luma <= 1.0,
            "expected capped sharpen at low bpp to avoid artifact amplification, got {}",
            s.unsharp_luma
        );
    }

    #[test]
    fn filter_strengths_are_continuous_across_thresholds() {
        // Test just above and below the high bpp threshold (0.15).
        // 720p30 at 1200 kbps → bpp = 1_200_000 / (1280*720*30) = 0.0434 (low)
        let low = compute_filter_strengths(1_200, 1280, 720, 30.0);
        // 1080p30 at 8000 kbps → bpp = 8_000_000 / (1920*1080*30) = 0.1286 (medium)
        let medium = compute_filter_strengths(8_000, 1920, 1080, 30.0);
        // 1080p30 at 15000 kbps → bpp = 15_000_000 / (1920*1080*30) = 0.2411 (high)
        let high = compute_filter_strengths(15_000, 1920, 1080, 30.0);

        // Denoising should be off at high bpp.
        assert_eq!(high.hqdn3d_luma_spatial, 0.0);
        // Denoising should be on at medium and low bpp.
        assert!(medium.hqdn3d_luma_spatial > 0.0);
        assert!(low.hqdn3d_luma_spatial > 0.0);
    }

    #[test]
    fn very_low_bitrate_caps_sharpening() {
        // 4K60 at 100 kbps → extremely low bpp
        let s = compute_filter_strengths(100, 3840, 2160, 60.0);
        assert!(
            s.unsharp_luma < 1.5,
            "sharpening must be capped at very low bpp, got {}",
            s.unsharp_luma
        );
    }

    #[test]
    fn zero_fps_does_not_panic() {
        // fps.max(1.0) prevents division by zero, resulting in very high bpp.
        let s = compute_filter_strengths(5_000, 1920, 1080, 0.0);
        // 5000 kbps at 1080p/1fps → bpp ≈ 2.4 → high bpp, no denoising
        assert_eq!(s.hqdn3d_luma_spatial, 0.0);
    }
}
