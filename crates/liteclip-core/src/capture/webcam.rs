//! Webcam capture via DirectShow (`dshow`) through libavformat (requires `ffmpeg` feature).
//!
//! Frames use QPC timestamps so PTS aligns with DXGI desktop capture.

#[cfg(all(feature = "ffmpeg", windows))]
mod imp {
    use anyhow::{bail, Context, Result};
    use bytes::Bytes;
    use crossbeam::channel::Sender;
    use ffmpeg_next as ffmpeg;
    use ffmpeg::format::{context::Input, Pixel};
    use ffmpeg::media::Type;
    use ffmpeg::software::scaling::{Context as ScalingContext, flag::Flags};
    use ffmpeg::util::frame::video::Video;
    use std::ffi::CString;
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::thread::JoinHandle;
    use std::time::Duration;
    use tracing::{info, warn};
    use windows::Win32::System::Performance::QueryPerformanceCounter;

    use crate::media::CapturedFrame;

    pub struct WebcamCapture {
        join: Option<JoinHandle<Result<()>>>,
        running: bool,
        stop_flag: Arc<AtomicBool>,
    }

    fn parse_dshow_video_list(stderr_text: &str) -> Vec<String> {
        let mut in_video_section = false;
        let mut out = Vec::new();
        for line in stderr_text.lines() {
            let line = line.trim_end();
            if line.contains("DirectShow video devices") {
                in_video_section = true;
                continue;
            }
            if line.contains("DirectShow audio devices") {
                break;
            }
            if in_video_section && line.contains("Alternative name") {
                continue;
            }
            if in_video_section && line.contains('"') {
                if let Some(start) = line.find('"') {
                    if let Some(end) = line[start + 1..].find('"') {
                        let name = line[start + 1..start + 1 + end].to_string();
                        out.push(name);
                    }
                }
            }
        }
        out
    }

    /// Lists DirectShow video device names (same source as capture auto-pick).
    pub fn list_dshow_video_devices() -> Result<Vec<String>> {
        let ffmpeg = crate::output::functions::ffmpeg_executable_path();
        let out = std::process::Command::new(&ffmpeg)
            .args([
                "-hide_banner",
                "-f",
                "dshow",
                "-list_devices",
                "true",
                "-i",
                "dummy",
            ])
            .output()
            .with_context(|| format!("failed to run {:?} for dshow list", ffmpeg))?;
        let text = String::from_utf8_lossy(&out.stderr);
        Ok(parse_dshow_video_list(&text))
    }

    impl WebcamCapture {
        pub fn new() -> Self {
            Self {
                join: None,
                running: false,
                stop_flag: Arc::new(AtomicBool::new(false)),
            }
        }

        pub fn stop_flag(&self) -> Arc<AtomicBool> {
            Arc::clone(&self.stop_flag)
        }

        fn qpc_timestamp() -> i64 {
            let mut qpc = 0i64;
            unsafe {
                let _ = QueryPerformanceCounter(&mut qpc);
            }
            qpc
        }

        fn open_dshow(device_name: &str) -> Result<Input> {
            ffmpeg::device::register_all();
            let fmt_name = CString::new("dshow").expect("dshow");
            let fmt_ptr = unsafe { ffmpeg::ffi::av_find_input_format(fmt_name.as_ptr()) };
            if fmt_ptr.is_null() {
                bail!("av_find_input_format(dshow) returned null");
            }
            let format = ffmpeg::format::format::Format::Input(unsafe {
                ffmpeg::format::format::Input::wrap(fmt_ptr as *mut _)
            });
            let url = format!("video={}", device_name);
            let ctx = ffmpeg::format::open(Path::new(&url), &format)
                .map_err(|e| anyhow::anyhow!("dshow open {}: {}", url, e))?;
            Ok(ctx.input())
        }

        fn pick_device(preferred: &str) -> Result<String> {
            if !preferred.trim().is_empty() {
                return Ok(preferred.trim().to_string());
            }
            list_dshow_video_devices()?
                .into_iter()
                .next()
                .ok_or_else(|| anyhow::anyhow!("no DirectShow video devices found"))
        }

        fn run_capture(
            device_name: String,
            target_w: u32,
            target_h: u32,
            target_fps: u32,
            frame_tx: Sender<CapturedFrame>,
            stop_flag: Arc<AtomicBool>,
        ) -> Result<()> {
            let mut input = Self::open_dshow(&device_name)?;
            let stream = input
                .streams()
                .best(Type::Video)
                .ok_or_else(|| anyhow::anyhow!("no video stream in dshow device"))?;
            let video_stream_index = stream.index();
            let context_decoder =
                ffmpeg::codec::context::Context::from_parameters(stream.parameters())?;
            let mut decoder = context_decoder.decoder().video()?;

            let mut scaler = ScalingContext::get(
                decoder.format(),
                decoder.width(),
                decoder.height(),
                Pixel::BGRA,
                target_w,
                target_h,
                Flags::BILINEAR,
            )?;

            let frame_duration = Duration::from_secs_f64(1.0 / target_fps.max(1) as f64);
            let mut last_send = std::time::Instant::now()
                .checked_sub(frame_duration)
                .unwrap_or_else(std::time::Instant::now);

            while !stop_flag.load(Ordering::Relaxed) {
                for (stream, packet) in input.packets() {
                    if stop_flag.load(Ordering::Relaxed) {
                        return Ok(());
                    }
                    if stream.index() != video_stream_index {
                        continue;
                    }
                    if decoder.send_packet(&packet).is_err() {
                        continue;
                    }
                    let mut decoded = Video::empty();
                    while decoder.receive_frame(&mut decoded).is_ok() {
                        let now = std::time::Instant::now();
                        if now.duration_since(last_send) < frame_duration {
                            continue;
                        }
                        last_send = now;

                        let mut bgra = Video::empty();
                        scaler.run(&decoded, &mut bgra)?;
                        let stride = bgra.stride(0) as usize;
                        let fw = bgra.width() as usize;
                        let fh = bgra.height() as usize;
                        let row_bytes = fw * 4;
                        let data = bgra.data(0);
                        let mut packed = Vec::with_capacity(row_bytes * fh);
                        for row in 0..fh {
                            let start = row * stride;
                            packed.extend_from_slice(&data[start..start + row_bytes]);
                        }

                        let ts = Self::qpc_timestamp();
                        let frame = CapturedFrame {
                            bgra: Bytes::from(packed),
                            #[cfg(windows)]
                            d3d11: None,
                            timestamp: ts,
                            resolution: (target_w, target_h),
                        };
                        if frame_tx.send(frame).is_err() {
                            return Ok(());
                        }
                    }
                }
                if stop_flag.load(Ordering::Relaxed) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            Ok(())
        }

        pub fn start_webcam_with_options(
            &mut self,
            preferred_device: &str,
            width: u32,
            height: u32,
            fps: u32,
            frame_tx: Sender<CapturedFrame>,
        ) -> Result<()> {
            if self.running {
                return Ok(());
            }
            self.stop_flag.store(false, Ordering::SeqCst);
            let device = Self::pick_device(preferred_device)?;
            info!("Webcam: using device {:?}", device);

            let frame_tx = frame_tx;
            let stop_flag = Arc::clone(&self.stop_flag);
            let join = std::thread::Builder::new()
                .name("webcam-dshow".to_string())
                .spawn(move || {
                    let r = Self::run_capture(device, width, height, fps, frame_tx, stop_flag);
                    if let Err(e) = &r {
                        warn!("Webcam capture ended: {:#}", e);
                    }
                    r
                })?;

            self.join = Some(join);
            self.running = true;
            Ok(())
        }

        pub fn stop(&mut self) {
            self.running = false;
            self.stop_flag.store(true, Ordering::SeqCst);
            if let Some(j) = self.join.take() {
                let _ = j.join();
            }
        }

        pub fn is_running(&self) -> bool {
            self.running
        }
    }

    impl Default for WebcamCapture {
        fn default() -> Self {
            Self::new()
        }
    }
}

#[cfg(all(feature = "ffmpeg", windows))]
pub use imp::{list_dshow_video_devices, WebcamCapture};

#[cfg(not(all(feature = "ffmpeg", windows)))]
pub mod stub {
    use anyhow::{bail, Result};
    use crossbeam::channel::Sender;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;
    use crate::media::CapturedFrame;

    pub struct WebcamCapture;

    pub fn list_dshow_video_devices() -> Result<Vec<String>> {
        Ok(Vec::new())
    }

    impl WebcamCapture {
        pub fn new() -> Self {
            Self
        }
        pub fn stop_flag(&self) -> Arc<AtomicBool> {
            Arc::new(AtomicBool::new(false))
        }
        pub fn start_webcam_with_options(
            &mut self,
            _preferred_device: &str,
            _width: u32,
            _height: u32,
            _fps: u32,
            _frame_tx: Sender<CapturedFrame>,
        ) -> Result<()> {
            bail!("webcam requires Windows and the ffmpeg feature")
        }
        pub fn stop(&mut self) {}
        pub fn is_running(&self) -> bool {
            false
        }
    }
}

#[cfg(not(all(feature = "ffmpeg", windows)))]
pub use stub::{list_dshow_video_devices, WebcamCapture};
