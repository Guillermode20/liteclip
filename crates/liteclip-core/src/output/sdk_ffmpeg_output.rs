//! Output helpers using linked `ffmpeg-next` (SDK backend only; no `ffmpeg.exe`).

use anyhow::{Context, Result};
use ffmpeg_next as ffmpeg;
use image::{DynamicImage, RgbaImage};
use std::path::Path;
use tracing::{debug, info, warn};

use super::video_file::VideoFileMetadata;

/// Copy a hardware-backed frame to a normal software frame so swscale can read pixels.
unsafe fn transfer_hw_frame_to_sw(
    src: &ffmpeg::util::frame::video::Video,
) -> Result<ffmpeg::util::frame::video::Video> {
    let mut dst = ffmpeg::util::frame::video::Video::empty();
    let ret = av_hwframe_transfer_data(dst.as_mut_ptr(), src.as_ptr(), 0);
    if ret < 0 {
        anyhow::bail!("av_hwframe_transfer_data failed ({ret})");
    }
    Ok(dst)
}

unsafe extern "C" {
    fn av_hwframe_transfer_data(
        dst: *mut ffmpeg::ffi::AVFrame,
        src: *const ffmpeg::ffi::AVFrame,
        flags: i32,
    ) -> i32;
}

fn pixel_format_is_hw(fmt: ffmpeg::format::Pixel) -> bool {
    use ffmpeg::format::Pixel::*;
    matches!(
        fmt,
        QSV | MMAL | CUDA | D3D11VA_VLD | D3D11 | MEDIACODEC | VIDEOTOOLBOX | DRM_PRIME
    )
}

fn frame_time_seconds(
    frame: &ffmpeg::util::frame::video::Video,
    time_base: ffmpeg::Rational,
) -> Option<f64> {
    let ts = frame.timestamp().or_else(|| frame.pts())?;
    let num = f64::from(time_base.numerator());
    let den = f64::from(time_base.denominator());
    if den <= 0.0 {
        return None;
    }
    Some(ts as f64 * num / den)
}

/// Remux MP4/MOV streams without re-encoding (copy), optionally moving the moov atom for web.
pub fn remux_fragmented_mp4(input_path: &Path, output_path: &Path, faststart: bool) -> Result<()> {
    let mut ictx = ffmpeg::format::input(input_path)
        .with_context(|| format!("failed to open input for remux: {:?}", input_path.display()))?;
    let mut octx = ffmpeg::format::output(output_path).with_context(|| {
        format!(
            "failed to create output for remux: {:?}",
            output_path.display()
        )
    })?;

    let mut stream_mapping = vec![0i32; ictx.nb_streams() as usize];
    let mut ist_time_bases = vec![ffmpeg::Rational(0, 1); ictx.nb_streams() as usize];
    let mut ost_index = 0i32;
    for (ist_index, ist) in ictx.streams().enumerate() {
        let ist_medium = ist.parameters().medium();
        if ist_medium != ffmpeg::media::Type::Audio
            && ist_medium != ffmpeg::media::Type::Video
            && ist_medium != ffmpeg::media::Type::Subtitle
        {
            stream_mapping[ist_index] = -1;
            continue;
        }
        stream_mapping[ist_index] = ost_index;
        ist_time_bases[ist_index] = ist.time_base();
        ost_index += 1;
        let mut ost = octx
            .add_stream(ffmpeg::encoder::find(ffmpeg::codec::Id::None))
            .with_context(|| "failed to add output stream")?;
        ost.set_parameters(ist.parameters());
        unsafe {
            (*ost.parameters().as_mut_ptr()).codec_tag = 0;
        }
    }

    octx.set_metadata(ictx.metadata().to_owned());

    if faststart {
        let mut opts = ffmpeg::Dictionary::new();
        opts.set("movflags", "+faststart");
        octx.write_header_with(opts)
            .with_context(|| "failed to write MP4 header (faststart)")?;
    } else {
        octx.write_header()
            .with_context(|| "failed to write MP4 header")?;
    }

    for (stream, mut packet) in ictx.packets() {
        let ist_index = stream.index();
        let ost_index = stream_mapping[ist_index];
        if ost_index < 0 {
            continue;
        }
        let ost = octx
            .stream(ost_index as usize)
            .context("missing mapped output stream")?;
        packet.rescale_ts(ist_time_bases[ist_index], ost.time_base());
        packet.set_position(-1);
        packet.set_stream(ost_index as usize);
        packet
            .write_interleaved(&mut octx)
            .with_context(|| "failed writing packet during remux")?;
    }

    octx.write_trailer()
        .with_context(|| "failed to finalize remux output")?;
    info!(
        "Remuxed {:?} -> {:?} (linked libav)",
        input_path, output_path
    );
    Ok(())
}

/// Probe duration, resolution, fps, and audio presence using libavformat.
pub fn probe_video_file(path: &Path) -> Result<VideoFileMetadata> {
    let ictx = ffmpeg::format::input(path)
        .with_context(|| format!("failed to open {:?} for probe", path.display()))?;

    let duration_secs = ictx.duration() as f64 / f64::from(ffmpeg::ffi::AV_TIME_BASE);

    let video = ictx
        .streams()
        .best(ffmpeg::media::Type::Video)
        .context("no video stream in file")?;
    let context = ffmpeg::codec::context::Context::from_parameters(video.parameters())?;
    let decoder = context.decoder().video()?;
    let width = decoder.width();
    let height = decoder.height();
    let rate = video.rate();
    let raw_fps = if rate.denominator() > 0 {
        rate.numerator() as f64 / rate.denominator() as f64
    } else {
        0.0
    };
    let fps = super::video_file::normalize_output_fps(raw_fps, 60.0);
    if (raw_fps - fps).abs() > f64::EPSILON {
        warn!(
            path = %path.display(),
            raw_fps,
            sanitized_fps = fps,
            "Ignoring unreasonable FPS reported by container"
        );
    }
    let has_audio = ictx.streams().best(ffmpeg::media::Type::Audio).is_some();

    Ok(VideoFileMetadata {
        duration_secs,
        width,
        height,
        has_audio,
        fps,
    })
}

/// Decode one frame at `timestamp_secs`, scale to `max_width`, return RGBA image.
pub fn extract_preview_frame(
    video_path: &Path,
    timestamp_secs: f64,
    max_width: u32,
) -> Result<RgbaImage> {
    let mut ictx = ffmpeg::format::input(video_path).with_context(|| {
        format!(
            "failed to open {:?} for preview frame",
            video_path.display()
        )
    })?;

    let video_stream_index = ictx
        .streams()
        .best(ffmpeg::media::Type::Video)
        .context("no video stream")?
        .index();

    let ts = (timestamp_secs * f64::from(ffmpeg::ffi::AV_TIME_BASE)) as i64;
    let _ = ictx.seek(ts, ..);

    let stream = ictx
        .stream(video_stream_index)
        .context("missing video stream after seek")?;
    let time_base = stream.time_base();
    let context_decoder = ffmpeg::codec::context::Context::from_parameters(stream.parameters())?;
    let mut decoder = context_decoder.decoder().video()?;

    let target_w = max_width.max(1);

    let mut scaler: Option<ffmpeg::software::scaling::Context> = None;
    let mut decoded = ffmpeg::util::frame::video::Video::empty();
    let mut rgba_frame = ffmpeg::util::frame::video::Video::empty();

    // Scale `sw` to RGBA (width `target_w`) and copy packed RGBA into an `image` buffer.
    let mut scale_to_image = |sw: &ffmpeg::util::frame::video::Video| -> Result<RgbaImage> {
        let src_w = sw.width().max(1);
        let src_h = sw.height().max(1);
        let target_h = ((src_h as u64 * target_w as u64) / src_w as u64).max(1) as u32;
        if scaler.is_none()
            || scaler.as_ref().unwrap().input().format != sw.format()
            || scaler.as_ref().unwrap().input().width != src_w
            || scaler.as_ref().unwrap().input().height != src_h
        {
            scaler = Some(
                ffmpeg::software::scaling::Context::get(
                    sw.format(),
                    src_w,
                    src_h,
                    ffmpeg::format::Pixel::RGBA,
                    target_w,
                    target_h,
                    ffmpeg::software::scaling::flag::Flags::BILINEAR,
                )
                .context("failed to create scaler for preview")?,
            );
        }
        scaler
            .as_mut()
            .expect("scaler")
            .run(sw, &mut rgba_frame)
            .context("swscale preview")?;
        ffmpeg_frame_to_rgba(&rgba_frame)
    };

    let mut best_after_seek: Option<(f64, RgbaImage)> = None;

    let mut consider_frame =
        |decoded: &ffmpeg::util::frame::video::Video| -> Result<Option<RgbaImage>> {
            let t_src = frame_time_seconds(decoded, time_base);
            let rgb = if pixel_format_is_hw(decoded.format()) {
                let sw = unsafe { transfer_hw_frame_to_sw(decoded)? };
                scale_to_image(&sw)?
            } else {
                scale_to_image(decoded)?
            };

            match t_src {
                Some(t) if t + 0.12 >= timestamp_secs => Ok(Some(rgb)),
                Some(t) => {
                    if best_after_seek.as_ref().map_or(true, |(bt, _)| t > *bt) {
                        best_after_seek = Some((t, rgb));
                    }
                    Ok(None)
                }
                None => Ok(Some(rgb)),
            }
        };

    for (stream, packet) in ictx.packets() {
        if stream.index() == video_stream_index {
            decoder.send_packet(&packet)?;
            loop {
                match decoder.receive_frame(&mut decoded) {
                    Ok(()) => {
                        if let Some(img) = consider_frame(&decoded)? {
                            return Ok(img);
                        }
                    }
                    Err(_) => break,
                }
            }
        }
    }

    decoder.send_eof()?;
    loop {
        match decoder.receive_frame(&mut decoded) {
            Ok(()) => {
                if let Some(img) = consider_frame(&decoded)? {
                    return Ok(img);
                }
            }
            Err(_) => break,
        }
    }

    if let Some((_, img)) = best_after_seek.take() {
        return Ok(img);
    }

    anyhow::bail!("no frame decoded for preview at {:.3}s", timestamp_secs)
}

fn ffmpeg_frame_to_rgba(frame: &ffmpeg::util::frame::video::Video) -> Result<RgbaImage> {
    let w = frame.width();
    let h = frame.height();
    let linesize0 = unsafe { (*frame.as_ptr()).linesize[0] };
    let stride = linesize0.unsigned_abs() as usize;
    let data = frame.data(0);
    let row_bytes = (w as usize).saturating_mul(4);
    let mut raw: Vec<u8> = vec![0u8; row_bytes * h as usize];
    let top_offset = if linesize0 < 0 {
        stride.saturating_mul(h as usize - 1)
    } else {
        0
    };
    for y in 0..h {
        let row_start = top_offset.saturating_add((y as usize).saturating_mul(stride));
        let available = data.len().saturating_sub(row_start);
        let n = row_bytes.min(available);
        if n == 0 {
            continue;
        }
        let row_src = &data[row_start..row_start + n];
        let dst_off = (y as usize).saturating_mul(row_bytes);
        raw[dst_off..dst_off + n].copy_from_slice(row_src);
    }
    RgbaImage::from_raw(w, h, raw)
        .with_context(|| "failed to build RGBA image from decoder frame")
}

/// Gallery thumbnail at ~1s; same cache path scheme as the CLI implementation.
pub fn generate_thumbnail(video_path: &Path, save_directory: &Path) -> Result<std::path::PathBuf> {
    use std::hash::{Hash, Hasher};

    let cache_dir = save_directory.join(".cache");
    std::fs::create_dir_all(&cache_dir)
        .with_context(|| format!("Failed to create cache directory: {:?}", cache_dir))?;

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    video_path.hash(&mut hasher);
    let hash = hasher.finish();
    let thumb_path = cache_dir.join(format!("{:016x}.jpg", hash));

    if thumb_path.exists() {
        debug!("Thumbnail already exists: {:?}", thumb_path);
        return Ok(thumb_path);
    }

    let img = extract_preview_frame(video_path, 1.0, 320)?;
    // `image` JPEG encoder expects RGB8/L8; Rgba8 is unsupported for `.jpg`.
    let rgb = DynamicImage::ImageRgba8(img).into_rgb8();
    rgb.save(&thumb_path)
        .with_context(|| format!("failed to write thumbnail {:?}", thumb_path))?;
    info!("Generated thumbnail (linked libav): {:?}", thumb_path);
    Ok(thumb_path)
}
