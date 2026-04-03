//! Burn subtitles into a video using linked FFmpeg (`ffmpeg-next`) libavfilter — `buffer` → `subtitles` → `buffersink`, then libx265 + audio stream copy.

use anyhow::{Context, Result};
use ffmpeg_next as ffmpeg;
use std::path::Path;

use ffmpeg::{codec, decoder, encoder, filter, format, frame, media, Dictionary, Packet, Rational};

/// Re-encode video with the `subtitles` filter and copy audio. Uses the same FFmpeg DLLs as the rest of the app.
pub fn burn_subtitles_with_filter_graph(
    video_in: &Path,
    srt_path: &Path,
    video_out: &Path,
    force_style: Option<&str>,
) -> Result<()> {
    ffmpeg::init().map_err(|e| anyhow::anyhow!("ffmpeg init: {e}"))?;

    if filter::find("subtitles").is_none() {
        anyhow::bail!(
            "libavfilter has no `subtitles` filter (libass not linked in this FFmpeg build)"
        );
    }

    let spec = subtitle_filter_spec(srt_path, force_style)?;
    let mut ictx =
        ffmpeg::format::input(video_in).with_context(|| format!("open input {:?}", video_in))?;
    let mut octx = ffmpeg::format::output(video_out)
        .with_context(|| format!("create output {:?}", video_out))?;

    let video_ist = ictx
        .streams()
        .best(media::Type::Video)
        .context("no video stream")?;
    let video_ist_index = video_ist.index();
    let audio_ist_index = ictx.streams().best(media::Type::Audio).map(|s| s.index());

    let mut stream_mapping: Vec<isize> = vec![-1; ictx.nb_streams() as usize];
    let mut ist_time_bases = vec![Rational(0, 1); ictx.nb_streams() as usize];
    let mut ost_index = 0usize;

    let mut transcoder: Option<VideoSubtitleTranscoder> = None;

    for (ist_index, ist) in ictx.streams().enumerate() {
        ist_time_bases[ist_index] = ist.time_base();
        let medium = ist.parameters().medium();
        if medium == media::Type::Video && ist_index == video_ist_index {
            stream_mapping[ist_index] = ost_index as isize;
            transcoder = Some(VideoSubtitleTranscoder::new(
                &ist, &mut octx, ost_index, &spec,
            )?);
            ost_index += 1;
        } else if medium == media::Type::Audio && audio_ist_index == Some(ist_index) {
            stream_mapping[ist_index] = ost_index as isize;
            let mut ost = octx
                .add_stream(encoder::find(codec::Id::None).context("codec none")?)
                .context("add audio stream")?;
            ost.set_parameters(ist.parameters());
            unsafe {
                (*ost.parameters().as_mut_ptr()).codec_tag = 0;
            }
            ost_index += 1;
        } else {
            stream_mapping[ist_index] = -1;
        }
    }

    let mut transcoder = transcoder.context("video transcoder missing")?;

    let mut ost_time_bases = vec![Rational(0, 1); ost_index];

    let mut opts = Dictionary::new();
    opts.set("movflags", "+faststart");
    octx.write_header_with(opts)
        .context("write output header")?;

    for (ost_i, _) in octx.streams().enumerate() {
        ost_time_bases[ost_i] = octx.stream(ost_i).unwrap().time_base();
    }

    for (stream, mut pkt) in ictx.packets() {
        let ist_i = stream.index();
        let ost_i = stream_mapping[ist_i];
        if ost_i < 0 {
            continue;
        }
        if ist_i == video_ist_index {
            transcoder.send_packet_decode_filter_encode(
                &pkt,
                &mut octx,
                ost_time_bases[ost_i as usize],
            )?;
        } else {
            let ost_tb = ost_time_bases[ost_i as usize];
            pkt.rescale_ts(ist_time_bases[ist_i], ost_tb);
            pkt.set_position(-1);
            pkt.set_stream(ost_i as usize);
            pkt.write_interleaved(&mut octx)?;
        }
    }

    transcoder.flush(&mut octx, &ost_time_bases)?;

    octx.write_trailer().context("write trailer")?;
    Ok(())
}

fn subtitle_filter_spec(srt_path: &Path, force_style: Option<&str>) -> Result<String> {
    let abs = srt_path
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

struct VideoSubtitleTranscoder {
    ost_index: usize,
    decoder: decoder::Video,
    encoder: encoder::Video,
    filter: filter::Graph,
}

impl VideoSubtitleTranscoder {
    fn new(
        ist: &format::stream::Stream,
        octx: &mut format::context::Output,
        ost_index: usize,
        filter_spec: &str,
    ) -> Result<Self, ffmpeg::Error> {
        let decoder = ffmpeg::codec::context::Context::from_parameters(ist.parameters())?
            .decoder()
            .video()?;

        let global_header = octx
            .format()
            .flags()
            .contains(ffmpeg::format::flag::Flags::GLOBAL_HEADER);

        let codec = encoder::find_by_name("libx265").ok_or(ffmpeg::Error::EncoderNotFound)?;
        let mut ost = octx.add_stream(codec)?;
        let enc_ctx = ffmpeg::codec::context::Context::new_with_codec(codec);
        let mut encoder = enc_ctx.encoder().video()?;

        encoder.set_width(decoder.width());
        encoder.set_height(decoder.height());
        encoder.set_aspect_ratio(decoder.aspect_ratio());
        encoder.set_format(ffmpeg::format::Pixel::YUV420P);
        encoder.set_frame_rate(decoder.frame_rate());
        encoder.set_time_base(ist.time_base());
        if global_header {
            encoder.set_flags(codec::Flags::GLOBAL_HEADER);
        }

        let mut dict = Dictionary::new();
        dict.set("crf", "20");
        dict.set("preset", "medium");
        let opened = encoder.open_with(dict)?;
        ost.set_parameters(&opened);

        let filter = build_video_subtitle_graph(&decoder, filter_spec, &opened)?;

        Ok(Self {
            ost_index,
            decoder,
            encoder: opened,
            filter,
        })
    }

    fn send_packet_decode_filter_encode(
        &mut self,
        packet: &Packet,
        octx: &mut format::context::Output,
        ost_time_base: Rational,
    ) -> Result<(), ffmpeg::Error> {
        self.decoder.send_packet(packet)?;
        let mut decoded = frame::Video::empty();
        while self.decoder.receive_frame(&mut decoded).is_ok() {
            let ts = decoded.timestamp();
            decoded.set_pts(ts);
            self.filter
                .get("in")
                .ok_or(ffmpeg::Error::InvalidData)?
                .source()
                .add(&decoded)?;
            self.drain_filter(octx, ost_time_base)?;
        }
        Ok(())
    }

    fn drain_filter(
        &mut self,
        octx: &mut format::context::Output,
        ost_time_base: Rational,
    ) -> Result<(), ffmpeg::Error> {
        let mut filtered = frame::Video::empty();
        while self
            .filter
            .get("out")
            .ok_or(ffmpeg::Error::InvalidData)?
            .sink()
            .frame(&mut filtered)
            .is_ok()
        {
            self.encoder.send_frame(&filtered)?;
            self.receive_encoded(octx, ost_time_base)?;
        }
        Ok(())
    }

    fn receive_encoded(
        &mut self,
        octx: &mut format::context::Output,
        ost_time_base: Rational,
    ) -> Result<(), ffmpeg::Error> {
        let mut pkt = Packet::empty();
        while self.encoder.receive_packet(&mut pkt).is_ok() {
            pkt.set_stream(self.ost_index);
            pkt.rescale_ts(self.encoder.time_base(), ost_time_base);
            pkt.write_interleaved(octx)?;
        }
        Ok(())
    }

    fn flush(
        &mut self,
        octx: &mut format::context::Output,
        ost_time_bases: &[Rational],
    ) -> Result<(), ffmpeg::Error> {
        let ost_tb = ost_time_bases[self.ost_index];
        self.decoder.send_eof()?;
        let mut decoded = frame::Video::empty();
        while self.decoder.receive_frame(&mut decoded).is_ok() {
            let ts = decoded.timestamp();
            decoded.set_pts(ts);
            self.filter.get("in").unwrap().source().add(&decoded)?;
            self.drain_filter(octx, ost_tb)?;
        }
        self.filter.get("in").unwrap().source().flush()?;
        self.drain_filter(octx, ost_tb)?;

        self.encoder.send_eof()?;
        let mut pkt = Packet::empty();
        while self.encoder.receive_packet(&mut pkt).is_ok() {
            pkt.set_stream(self.ost_index);
            pkt.rescale_ts(self.encoder.time_base(), ost_tb);
            pkt.write_interleaved(octx)?;
        }
        Ok(())
    }
}

fn build_video_subtitle_graph(
    decoder: &decoder::Video,
    spec: &str,
    encoder: &encoder::Video,
) -> Result<filter::Graph, ffmpeg::Error> {
    let mut graph = filter::Graph::new();
    let tb = decoder.time_base();
    let args = format!(
        "video_size={}x{}:pix_fmt={}:time_base={}/{}:pixel_aspect={}/{}",
        decoder.width(),
        decoder.height(),
        decoder
            .format()
            .descriptor()
            .map(|d| d.name())
            .unwrap_or("yuv420p"),
        tb.numerator(),
        tb.denominator(),
        decoder.aspect_ratio().numerator(),
        decoder.aspect_ratio().denominator(),
    );
    graph.add(
        &filter::find("buffer").ok_or(ffmpeg::Error::InvalidData)?,
        "in",
        &args,
    )?;
    graph.add(
        &filter::find("buffersink").ok_or(ffmpeg::Error::InvalidData)?,
        "out",
        "",
    )?;
    {
        let mut out = graph.get("out").unwrap();
        out.set_pixel_format(encoder.format());
    }
    graph.output("in", 0)?.input("out", 0)?.parse(spec)?;
    graph.validate()?;
    Ok(graph)
}
