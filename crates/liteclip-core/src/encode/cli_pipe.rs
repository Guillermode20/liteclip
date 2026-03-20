//! Software H.264 encoder via external `ffmpeg` stdin/stdout (`ffmpeg-cli` feature only).

use crate::encode::{
    EncodeError, EncodeResult, EncodedPacket, Encoder, ResolvedEncoderConfig, StreamType,
};
use crossbeam::channel::{unbounded, Receiver, Sender};
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use tracing::{info, warn};

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x08000000;

pub struct CliPipeEncoder {
    config: ResolvedEncoderConfig,
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    reader: Option<JoinHandle<()>>,
    packet_tx: Sender<EncodedPacket>,
    packet_rx: Receiver<EncodedPacket>,
    frame_count: i64,
    running: bool,
    pts_queue: Arc<Mutex<VecDeque<i64>>>,
}

impl CliPipeEncoder {
    pub fn new(config: &ResolvedEncoderConfig) -> EncodeResult<Self> {
        let (packet_tx, packet_rx) = unbounded();
        Ok(Self {
            config: config.clone(),
            child: None,
            stdin: None,
            reader: None,
            packet_tx,
            packet_rx,
            frame_count: 0,
            running: false,
            pts_queue: Arc::new(Mutex::new(VecDeque::new())),
        })
    }

    fn spawn_ffmpeg(&mut self, width: u32, height: u32) -> EncodeResult<()> {
        let ffmpeg = crate::runtime::resolve_ffmpeg_executable();
        let fps = self.config.framerate.max(1);
        let gop = self.config.keyframe_interval_frames().max(1);
        let bitrate_kbps = self.config.bitrate_mbps.saturating_mul(1000).max(500);

        let mut cmd = Command::new(&ffmpeg);
        cmd.arg("-hide_banner")
            .arg("-loglevel")
            .arg("error")
            .arg("-f")
            .arg("rawvideo")
            .arg("-pix_fmt")
            .arg("bgra")
            .arg("-video_size")
            .arg(format!("{width}x{height}"))
            .arg("-framerate")
            .arg(fps.to_string())
            .arg("-i")
            .arg("pipe:0")
            .arg("-an")
            .arg("-c:v")
            .arg("libx264")
            .arg("-preset")
            .arg("ultrafast")
            .arg("-tune")
            .arg("zerolatency")
            .arg("-pix_fmt")
            .arg("yuv420p")
            .arg("-g")
            .arg(gop.to_string())
            .arg("-b:v")
            .arg(format!("{bitrate_kbps}k"))
            .arg("-maxrate")
            .arg(format!("{bitrate_kbps}k"))
            .arg("-bufsize")
            .arg(format!("{}k", bitrate_kbps.saturating_mul(2)))
            .arg("-f")
            .arg("h264")
            .arg("pipe:1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        let mut child = cmd
            .spawn()
            .map_err(|e| EncodeError::msg(format!("failed to spawn ffmpeg encoder: {e}")))?;

        let stdin = child.stdin.take().ok_or_else(|| {
            EncodeError::msg("ffmpeg stdin pipe unavailable for CLI encoder")
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            EncodeError::msg("ffmpeg stdout pipe unavailable for CLI encoder")
        })?;
        let stderr = child.stderr.take();

        if let Some(mut err) = stderr {
            thread::spawn(move || {
                let mut s = String::new();
                let _ = std::io::Read::read_to_string(&mut err, &mut s);
                if !s.trim().is_empty() {
                    warn!("ffmpeg encoder stderr: {}", s.trim());
                }
            });
        }

        let out_tx = self.packet_tx.clone();
        let pts_q = Arc::clone(&self.pts_queue);
        let reader = thread::spawn(move || {
            reader_loop(stdout, out_tx, pts_q);
        });

        self.stdin = Some(stdin);
        self.child = Some(child);
        self.reader = Some(reader);
        self.running = true;
        info!(
            "CLI pipe encoder started (libx264 {}x{} @ {} fps)",
            width, height, fps
        );
        Ok(())
    }
}

fn reader_loop(
    mut stdout: ChildStdout,
    packet_tx: Sender<EncodedPacket>,
    pts_queue: Arc<Mutex<VecDeque<i64>>>,
) {
    let mut buf = Vec::with_capacity(512 * 1024);
    let mut scratch = [0u8; 16384];
    loop {
        match stdout.read(&mut scratch) {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&scratch[..n]),
            Err(_) => break,
        }
        while let Some(au) = pop_next_access_unit(&mut buf) {
            emit_au(au, &packet_tx, &pts_queue);
        }
    }
    if !buf.is_empty() {
        let pts = pts_queue
            .lock()
            .ok()
            .and_then(|mut q| q.pop_front())
            .unwrap_or(0);
        let is_key = h264_is_keyframe(&buf);
        let pkt = EncodedPacket::new(buf, pts, pts, is_key, StreamType::Video);
        let _ = packet_tx.send(pkt);
    }
}

fn emit_au(
    au: Vec<u8>,
    packet_tx: &Sender<EncodedPacket>,
    pts_queue: &Arc<Mutex<VecDeque<i64>>>,
) {
    let pts = pts_queue
        .lock()
        .ok()
        .and_then(|mut q| q.pop_front())
        .unwrap_or(0);
    let is_key = h264_is_keyframe(&au);
    let pkt = EncodedPacket::new(au, pts, pts, is_key, StreamType::Video);
    let _ = packet_tx.send(pkt);
}

/// Split Annex-B stream into access units (split before second VCL NAL).
fn pop_next_access_unit(buf: &mut Vec<u8>) -> Option<Vec<u8>> {
    let starts = find_all_start_code_positions(buf);
    if starts.is_empty() {
        return None;
    }
    let mut vcl_positions: Vec<usize> = Vec::new();
    for &pos in &starts {
        let sc = if buf[pos..].starts_with(&[0, 0, 0, 1]) {
            4
        } else if buf[pos..].starts_with(&[0, 0, 1]) {
            3
        } else {
            continue;
        };
        let nal_start = pos + sc;
        if nal_start >= buf.len() {
            break;
        }
        let nal_type = buf[nal_start] & 0x1f;
        if matches!(nal_type, 1 | 5) {
            vcl_positions.push(pos);
        }
    }
    if vcl_positions.len() < 2 {
        return None;
    }
    let split_at = vcl_positions[1];
    Some(buf.drain(..split_at).collect())
}

fn find_all_start_code_positions(data: &[u8]) -> Vec<usize> {
    let mut v = Vec::new();
    let mut i = 0usize;
    while i + 3 < data.len() {
        if data[i..].starts_with(&[0, 0, 0, 1]) || data[i..].starts_with(&[0, 0, 1]) {
            v.push(i);
        }
        i += 1;
    }
    v
}

fn h264_is_keyframe(au: &[u8]) -> bool {
    let mut i = 0usize;
    while i + 4 < au.len() {
        let sc = if au[i..].starts_with(&[0, 0, 0, 1]) {
            4
        } else if au[i..].starts_with(&[0, 0, 1]) {
            3
        } else {
            i += 1;
            continue;
        };
        let nal = au.get(i + sc).map(|b| b & 0x1f);
        if nal == Some(5) {
            return true;
        }
        i += 1;
    }
    false
}

impl Encoder for CliPipeEncoder {
    fn init(&mut self, config: &ResolvedEncoderConfig) -> EncodeResult<()> {
        self.config = config.clone();
        let (w, h) = if config.use_native_resolution {
            (config.resolution.0, config.resolution.1)
        } else {
            config.resolution
        };
        if w == 0 || h == 0 {
            return Err(EncodeError::msg("invalid resolution for CLI encoder"));
        }
        self.spawn_ffmpeg(w, h)
    }

    fn encode_frame(&mut self, frame: &crate::media::CapturedFrame) -> EncodeResult<()> {
        if frame.bgra.is_empty() {
            return Err(EncodeError::msg(
                "ffmpeg-cli encoder requires CPU BGRA pixels; enable CPU readback in settings",
            ));
        }
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| EncodeError::msg("CLI encoder not initialized"))?;

        let (w, h) = frame.resolution;
        let expected = (w as usize) * (h as usize) * 4;
        if frame.bgra.len() < expected {
            return Err(EncodeError::msg(format!(
                "BGRA buffer too small: got {} need {}",
                frame.bgra.len(),
                expected
            )));
        }

        if let Ok(mut q) = self.pts_queue.lock() {
            q.push_back(frame.timestamp);
            while q.len() > 512 {
                q.pop_front();
            }
        }

        stdin
            .write_all(&frame.bgra[..expected])
            .map_err(|e| EncodeError::msg(format!("failed writing raw frame to ffmpeg: {e}")))?;
        stdin
            .flush()
            .map_err(|e| EncodeError::msg(format!("failed flush ffmpeg stdin: {e}")))?;

        self.frame_count += 1;
        Ok(())
    }

    fn flush(&mut self) -> EncodeResult<Vec<EncodedPacket>> {
        drop(self.stdin.take());
        if let Some(c) = self.child.take() {
            let _ = c.wait_with_output();
        }
        if let Some(r) = self.reader.take() {
            let _ = r.join();
        }
        let mut out = Vec::new();
        while let Ok(p) = self.packet_rx.try_recv() {
            out.push(p);
        }
        self.running = false;
        Ok(out)
    }

    fn packet_rx(&self) -> Receiver<EncodedPacket> {
        self.packet_rx.clone()
    }

    fn is_running(&self) -> bool {
        self.running
    }
}
