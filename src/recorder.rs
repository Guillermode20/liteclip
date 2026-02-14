use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::Instant;
use tempfile::TempDir;

use crate::settings::Settings;

/// Current state of the recorder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecorderState {
    Idle,
    Recording,
    Saving,
}

/// Manages the FFmpeg subprocess and segment-based replay buffer.
pub struct Recorder {
    pub state: RecorderState,
    pub settings: Settings,
    pub ffmpeg_found: bool,
    pub last_saved_path: Option<PathBuf>,
    pub audio_devices: Vec<String>,

    child: Option<Child>,
    temp_dir: Option<TempDir>,
    started_at: Option<Instant>,
}

impl Recorder {
    /// Create a new Recorder with default settings.
    pub fn new() -> Self {
        // Check if FFmpeg is available on PATH
        let ffmpeg_found = Command::new("ffmpeg")
            .arg("-version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map(|mut c| c.wait().is_ok())
            .unwrap_or(false);

        // Detect audio devices via FFmpeg
        let audio_devices = detect_audio_devices();

        let mut settings = Settings::default();

        // Auto-select first audio device if available
        if !audio_devices.is_empty() {
            settings.audio_device = Some(audio_devices[0].clone());
        }

        Self {
            state: RecorderState::Idle,
            settings,
            ffmpeg_found,
            last_saved_path: None,
            audio_devices,
            child: None,
            temp_dir: None,
            started_at: None,
        }
    }

    /// How many segment files to keep (buffer / 10s segments).
    fn segment_wrap(&self) -> u64 {
        self.settings.buffer_seconds / 10
    }

    /// How long the buffer has been recording.
    pub fn elapsed_seconds(&self) -> u64 {
        self.started_at
            .map(|t| t.elapsed().as_secs())
            .unwrap_or(0)
    }

    /// Start recording the desktop into rolling segments.
    pub fn start(&mut self) -> Result<(), String> {
        if !self.ffmpeg_found {
            return Err("FFmpeg not found on PATH. Please install FFmpeg.".into());
        }
        if self.state == RecorderState::Recording {
            return Err("Already recording.".into());
        }

        // Create temp dir for segments
        let temp_dir =
            TempDir::new().map_err(|e| format!("Failed to create temp dir: {}", e))?;
        let segment_pattern = temp_dir.path().join("seg_%04d.mp4");

        let segment_duration: u64 = 10;
        let force_kf = format!("expr:gte(t,n_forced*{})", segment_duration);
        let fps = self.settings.framerate.value().to_string();
        let crf = self.settings.quality.crf().to_string();
        let preset = self.settings.quality.preset();
        let wrap = self.segment_wrap().to_string();

        let mut args: Vec<String> = Vec::new();

        // Video input: gdigrab
        args.extend([
            "-f".into(), "gdigrab".into(),
            "-framerate".into(), fps.clone(),
            "-i".into(), "desktop".into(),
        ]);

        // Audio input: dshow (if enabled and device available)
        if self.settings.capture_audio {
            if let Some(ref device) = self.settings.audio_device {
                args.extend([
                    "-f".into(), "dshow".into(),
                    "-i".into(), format!("audio={}", device),
                ]);
            }
        }

        // Build video filter chain
        let mut vfilters = Vec::new();
        if let Some(scale) = self.settings.resolution.scale_filter() {
            vfilters.push(scale.to_string());
        }
        if !vfilters.is_empty() {
            args.extend(["-vf".into(), vfilters.join(",")]);
        }

        // Video encoding
        args.extend([
            "-c:v".into(), "libx264".into(),
            "-preset".into(), preset.into(),
            "-crf".into(), crf,
            "-pix_fmt".into(), "yuv420p".into(),
            "-force_key_frames".into(), force_kf,
        ]);

        // Audio encoding (if audio is being captured)
        if self.settings.capture_audio && self.settings.audio_device.is_some() {
            args.extend([
                "-c:a".into(), "aac".into(),
                "-b:a".into(), "128k".into(),
            ]);
        }

        // Segment muxer
        args.extend([
            "-f".into(), "segment".into(),
            "-segment_time".into(), segment_duration.to_string(),
            "-segment_wrap".into(), wrap,
            "-reset_timestamps".into(), "1".into(),
            "-y".into(),
        ]);

        args.push(segment_pattern.to_str().unwrap().into());

        let child = Command::new("ffmpeg")
            .args(&args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| format!("Failed to spawn FFmpeg: {}", e))?;

        self.child = Some(child);
        self.temp_dir = Some(temp_dir);
        self.started_at = Some(Instant::now());
        self.state = RecorderState::Recording;

        Ok(())
    }

    /// Stop recording and terminate the FFmpeg process.
    pub fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            // Send 'q' to FFmpeg stdin for graceful shutdown
            if let Some(ref mut stdin) = child.stdin {
                let _ = stdin.write_all(b"q");
            }
            let _ = child.wait();
        }
        self.state = RecorderState::Idle;
        self.started_at = None;
    }

    /// Auto-save the current replay buffer to the configured output directory.
    /// Returns the path to the saved clip.
    pub fn save_clip_auto(&mut self) -> Result<PathBuf, String> {
        let output_path = self.auto_output_path();
        self.save_clip(&output_path)
    }

    /// Save the current replay buffer to a specific path.
    pub fn save_clip(&mut self, output_path: &Path) -> Result<PathBuf, String> {
        let was_recording = self.state == RecorderState::Recording;
        self.state = RecorderState::Saving;

        // Stop FFmpeg so all segments are flushed
        if let Some(mut child) = self.child.take() {
            if let Some(ref mut stdin) = child.stdin {
                let _ = stdin.write_all(b"q");
            }
            let _ = child.wait();
        }

        let temp_path = self.temp_dir.as_ref()
            .ok_or("No temp directory — nothing recorded yet.")?
            .path()
            .to_path_buf();

        // Gather segment files sorted by modification time
        let mut segments: Vec<PathBuf> = fs::read_dir(&temp_path)
            .map_err(|e| format!("Failed to read temp dir: {}", e))?
            .filter_map(|entry| {
                let entry = entry.ok()?;
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("mp4") {
                    Some(path)
                } else {
                    None
                }
            })
            .collect();

        if segments.is_empty() {
            self.state = if was_recording {
                RecorderState::Recording
            } else {
                RecorderState::Idle
            };
            return Err("No segments found. Record for a few seconds first.".into());
        }

        // Sort by modification time (oldest first)
        segments.sort_by_key(|p| {
            fs::metadata(p)
                .and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
        });

        // Create the concat list file
        let concat_list_path = temp_path.join("concat_list.txt");
        let mut concat_file = fs::File::create(&concat_list_path)
            .map_err(|e| format!("Failed to create concat list: {}", e))?;

        for seg in &segments {
            writeln!(concat_file, "file '{}'", seg.display())
                .map_err(|e| format!("Failed to write concat list: {}", e))?;
        }
        drop(concat_file);

        // Ensure output directory exists
        if let Some(parent) = output_path.parent() {
            let _ = fs::create_dir_all(parent);
        }

        // Concatenate segments into final clip
        let concat_result = Command::new("ffmpeg")
            .args([
                "-f", "concat",
                "-safe", "0",
                "-i",
            ])
            .arg(concat_list_path.to_str().unwrap())
            .args(["-c", "copy", "-y"])
            .arg(output_path.to_str().unwrap())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .map_err(|e| format!("Failed to run FFmpeg concat: {}", e))?;

        if !concat_result.status.success() {
            return Err("FFmpeg concat failed. Check segment files.".into());
        }

        self.last_saved_path = Some(output_path.to_path_buf());

        // Restart recording if it was active
        if was_recording {
            self.temp_dir = None;
            self.started_at = None;
            let _ = self.start();
        } else {
            self.state = RecorderState::Idle;
        }

        Ok(output_path.to_path_buf())
    }

    /// Generate an auto-save output path with timestamp.
    fn auto_output_path(&self) -> PathBuf {
        let now = chrono::Local::now();
        let filename = format!("LiteClip_{}.mp4", now.format("%Y-%m-%d_%H-%M-%S"));
        self.settings.output_dir.join(filename)
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Detect available audio recording devices via FFmpeg.
fn detect_audio_devices() -> Vec<String> {
    let output = Command::new("ffmpeg")
        .args(["-list_devices", "true", "-f", "dshow", "-i", "dummy"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output();

    let output = match output {
        Ok(o) => o,
        Err(_) => return Vec::new(),
    };

    // FFmpeg lists devices on stderr
    let stderr = String::from_utf8_lossy(&output.stderr);
    let mut devices = Vec::new();
    let mut in_audio_section = false;

    for line in stderr.lines() {
        if line.contains("DirectShow audio devices") {
            in_audio_section = true;
            continue;
        }
        if line.contains("DirectShow video devices") {
            in_audio_section = false;
            continue;
        }
        if in_audio_section {
            // Lines look like: [dshow @ ...] "Device Name"
            if let Some(start) = line.find('"') {
                if let Some(end) = line[start + 1..].find('"') {
                    let name = &line[start + 1..start + 1 + end];
                    // Skip "Alternative name" lines
                    if !name.starts_with('@') {
                        devices.push(name.to_string());
                    }
                }
            }
        }
    }

    devices
}
