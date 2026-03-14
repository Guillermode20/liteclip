use anyhow::Context;
use eframe::egui;
use image::RgbaImage;
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use tokio::sync::mpsc::Sender as TokioSender;
use tracing::{info, warn};

mod browser;
mod decode_pipeline;
mod editor;

use decode_pipeline::PlaybackController;

use crate::config::Config;
use crate::gui::manager::{show_toast, ToastKind};
use crate::output::{
    generate_thumbnail, probe_video_file, spawn_clip_export, ClipExportRequest, ClipExportUpdate,
    TimeRange, VideoFileMetadata,
};
use crate::platform::AppEvent;

const ALL_GAMES_FILTER: &str = "All Games";
const DEFAULT_TARGET_SIZE_MB: u32 = 25;
const DEFAULT_AUDIO_BITRATE_KBPS: u32 = 128;
const PREVIEW_FRAME_WIDTH: u32 = 640;
const PREVIEW_FRAME_INTERVAL_SECS: f64 = 0.25;
const MIN_RANGE_SECS: f64 = 0.1;
const EDITOR_SIDEBAR_WIDTH: f32 = 340.0;
const EDITOR_SIDEBAR_MIN_WIDTH: f32 = 280.0;
const EDITOR_STACK_BREAKPOINT: f32 = 960.0;

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x08000000;

pub fn show_gallery_gui(event_tx: TokioSender<AppEvent>) {
    crate::gui::manager::send_gui_message(crate::gui::manager::GuiMessage::ShowGallery(event_tx));
}

#[derive(Clone)]
struct VideoEntry {
    path: PathBuf,
    save_root: PathBuf,
    game: String,
    filename: String,
    size_mb: f64,
    modified: SystemTime,
    metadata: VideoFileMetadata,
}

struct ThumbnailResult {
    video_path: PathBuf,
    image: Option<RgbaImage>,
    error: Option<String>,
}

#[derive(Clone, Copy)]
struct SnippetSegment {
    start_secs: f64,
    end_secs: f64,
    enabled: bool,
}

impl SnippetSegment {
    fn duration_secs(self) -> f64 {
        (self.end_secs - self.start_secs).max(0.0)
    }
}

struct ExportState {
    progress_rx: Receiver<ClipExportUpdate>,
    cancel_flag: Arc<AtomicBool>,
    progress: f32,
    message: String,
}

struct EditorState {
    video: VideoEntry,
    current_time_secs: f64,
    is_playing: bool,
    preview_final: bool,
    last_tick: Instant,
    cut_points: Vec<f64>,
    snippet_enabled: Vec<bool>,
    selected_cut_point: Option<usize>,
    target_size_mb: u32,
    preview_texture: Option<egui::TextureHandle>,
    preview_frame_in_flight: bool,
    pending_preview_request: Option<f64>,
    last_requested_preview_time: Option<f64>,
    status_message: Option<String>,
    error_message: Option<String>,
    export_state: Option<ExportState>,
    export_output: Option<PathBuf>,
    playback: PlaybackController,
    // Scrubbing optimization fields
    scrub_state: ScrubState,
    last_scrub_time: Option<Instant>,
    last_scrub_position: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ScrubState {
    Idle,
    DraggingFast,   // > 2x playback speed - use low res
    DraggingNormal, // Normal speed - use medium res
    DraggingSlow,   // < 0.5x speed - use high res
}

impl ScrubState {
    fn preview_width(&self) -> u32 {
        match self {
            ScrubState::Idle => 640,
            ScrubState::DraggingFast => 320, // Fast scrub = low res for speed
            ScrubState::DraggingNormal => 480, // Normal = medium res
            ScrubState::DraggingSlow => 640, // Slow = high res
        }
    }
}

impl EditorState {
    fn new(video: VideoEntry) -> Self {
        let target_size_mb = DEFAULT_TARGET_SIZE_MB
            .max(video.size_mb.round() as u32 / 2)
            .min(video.size_mb.ceil().max(1.0) as u32);
        let playback = PlaybackController::new(
            video.path.clone(),
            video.metadata.clone(),
            PREVIEW_FRAME_WIDTH,
        );
        Self {
            video,
            current_time_secs: 0.0,
            is_playing: false,
            preview_final: false,
            last_tick: Instant::now(),
            cut_points: Vec::new(),
            snippet_enabled: vec![true],
            selected_cut_point: None,
            target_size_mb,
            preview_texture: None,
            preview_frame_in_flight: false,
            pending_preview_request: None,
            last_requested_preview_time: None,
            status_message: None,
            error_message: None,
            export_state: None,
            export_output: None,
            playback,
            scrub_state: ScrubState::Idle,
            last_scrub_time: None,
            last_scrub_position: None,
        }
    }

    fn duration_secs(&self) -> f64 {
        self.video.metadata.duration_secs
    }

    fn kept_ranges(&self) -> Vec<TimeRange> {
        enabled_time_ranges(
            self.duration_secs(),
            &self.cut_points,
            &self.snippet_enabled,
        )
    }

    fn kept_duration_secs(&self) -> f64 {
        self.kept_ranges()
            .iter()
            .map(|range| range.duration_secs())
            .sum()
    }

    fn snippets(&self) -> Vec<SnippetSegment> {
        snippet_segments(
            self.duration_secs(),
            &self.cut_points,
            &self.snippet_enabled,
        )
    }

    fn has_active_export(&self) -> bool {
        self.export_state.is_some()
    }
}

pub struct ClipCompressApp {
    save_directory: PathBuf,
    cache_directory: PathBuf,
    videos_by_game: Vec<(String, Vec<VideoEntry>)>,
    filter_game: String,
    loaded: bool,
    scan_error: Option<String>,
    thumbnails: HashMap<PathBuf, egui::TextureHandle>,
    thumbnails_generating: HashSet<PathBuf>,
    thumbnail_tx: Sender<ThumbnailResult>,
    thumbnail_rx: Receiver<ThumbnailResult>,
    editor: Option<EditorState>,
}

pub type GalleryApp = ClipCompressApp;

impl ClipCompressApp {
    pub fn new(config: &Config, _event_tx: TokioSender<AppEvent>) -> Self {
        let save_directory = PathBuf::from(&config.general.save_directory);
        let cache_directory = save_directory.join(".cache");
        let (thumbnail_tx, thumbnail_rx) = mpsc::channel();

        Self {
            save_directory,
            cache_directory,
            videos_by_game: Vec::new(),
            filter_game: ALL_GAMES_FILTER.to_string(),
            loaded: false,
            scan_error: None,
            thumbnails: HashMap::new(),
            thumbnails_generating: HashSet::new(),
            thumbnail_tx,
            thumbnail_rx,
            editor: None,
        }
    }

    fn get_thumb_path(&self, video_path: &Path) -> PathBuf {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        video_path.hash(&mut hasher);
        self.cache_directory
            .join(format!("{:016x}.jpg", hasher.finish()))
    }

    fn scan_videos(&mut self, ctx: &egui::Context) {
        self.scan_error = None;
        self.videos_by_game.clear();
        self.thumbnails.clear();

        if !self.save_directory.exists() {
            self.loaded = true;
            return;
        }

        let mut paths = Vec::new();
        collect_video_paths(&self.save_directory, &self.cache_directory, &mut paths);
        let base_dir = self.save_directory.clone();

        let mut entries: Vec<VideoEntry> = paths
            .into_par_iter()
            .filter_map(|path| match Self::build_video_entry(&base_dir, path) {
                Ok(entry) => Some(entry),
                Err(err) => {
                    warn!("Skipping video during scan: {err:#}");
                    None
                }
            })
            .collect();

        entries.sort_by(|a, b| {
            a.game
                .cmp(&b.game)
                .then_with(|| b.modified.cmp(&a.modified))
                .then_with(|| a.filename.cmp(&b.filename))
        });

        let mut grouped: Vec<(String, Vec<VideoEntry>)> = Vec::new();
        for entry in entries {
            if let Some((game, videos)) = grouped.last_mut() {
                if *game == entry.game {
                    videos.push(entry);
                    continue;
                }
            }
            grouped.push((entry.game.clone(), vec![entry]));
        }

        grouped.sort_by(|a, b| {
            if a.0 == "Desktop" {
                std::cmp::Ordering::Less
            } else if b.0 == "Desktop" {
                std::cmp::Ordering::Greater
            } else {
                a.0.cmp(&b.0)
            }
        });

        self.videos_by_game = grouped;
        if self
            .videos_by_game
            .iter()
            .all(|(game, _)| *game != self.filter_game)
        {
            self.filter_game = ALL_GAMES_FILTER.to_string();
        }
        self.loaded = true;
        self.load_cached_thumbnails(ctx);
    }

    fn build_video_entry(base_dir: &Path, path: PathBuf) -> anyhow::Result<VideoEntry> {
        let metadata = std::fs::metadata(&path)
            .map_err(anyhow::Error::from)
            .with_context(|| format!("Failed to read metadata for {:?}", path))?;
        let video_metadata = probe_video_file(&path)
            .with_context(|| format!("Failed to probe video file {:?}", path))?;
        let filename = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown.mp4".to_string());
        let relative = path.strip_prefix(base_dir).unwrap_or(&path);
        let game = relative
            .parent()
            .map(|parent| parent.to_string_lossy().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "Desktop".to_string());

        Ok(VideoEntry {
            path,
            save_root: base_dir.to_path_buf(),
            game,
            filename,
            size_mb: metadata.len() as f64 / (1024.0 * 1024.0),
            modified: metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            metadata: video_metadata,
        })
    }

    fn load_cached_thumbnails(&mut self, ctx: &egui::Context) {
        let mut cached = Vec::new();
        for video in self
            .videos_by_game
            .iter()
            .flat_map(|(_, videos)| videos.iter())
        {
            let thumb_path = self.get_thumb_path(&video.path);
            if !thumb_path.exists() || self.thumbnails.contains_key(&video.path) {
                continue;
            }
            if let Ok(image) = load_rgba_image_from_path(&thumb_path) {
                cached.push((video.path.clone(), video.filename.clone(), image));
            }
        }
        for (video_path, filename, image) in cached {
            self.insert_thumbnail_texture(ctx, video_path, &filename, image);
        }
    }

    fn insert_thumbnail_texture(
        &mut self,
        ctx: &egui::Context,
        video_path: PathBuf,
        texture_name: &str,
        image: RgbaImage,
    ) {
        let texture = ctx.load_texture(
            texture_name.to_string(),
            color_image_from_rgba(&image),
            egui::TextureOptions::LINEAR,
        );
        self.thumbnails.insert(video_path, texture);
    }

    fn schedule_thumbnail_generation(&mut self, video: &VideoEntry) {
        if self.thumbnails.contains_key(&video.path)
            || self.thumbnails_generating.contains(&video.path)
        {
            return;
        }

        self.thumbnails_generating.insert(video.path.clone());
        let tx = self.thumbnail_tx.clone();
        let video_path = video.path.clone();
        let save_directory = self.save_directory.clone();
        std::thread::spawn(move || {
            let result = generate_thumbnail(&video_path, &save_directory)
                .and_then(|thumb_path| load_rgba_image_from_path(&thumb_path));

            let message = match result {
                Ok(image) => ThumbnailResult {
                    video_path,
                    image: Some(image),
                    error: None,
                },
                Err(err) => ThumbnailResult {
                    video_path,
                    image: None,
                    error: Some(format!("{err:#}")),
                },
            };
            let _ = tx.send(message);
        });
    }

    fn queue_preview_request(&mut self, timestamp_secs: f64) {
        let Some(editor) = self.editor.as_mut() else {
            return;
        };

        if editor.is_playing && !editor.preview_final {
            editor.preview_frame_in_flight = false;
            editor.pending_preview_request = None;
            return;
        }

        let timestamp_secs = timestamp_secs.clamp(0.0, editor.duration_secs());

        // Skip if decoder is already processing a request
        if editor.playback.is_frame_request_in_flight() {
            editor.pending_preview_request = Some(timestamp_secs);
            return;
        }

        if editor.preview_frame_in_flight {
            editor.pending_preview_request = Some(timestamp_secs);
            return;
        }

        if let Some(last_requested) = editor.last_requested_preview_time {
            if editor.preview_texture.is_some() && (last_requested - timestamp_secs).abs() < 0.05 {
                return;
            }
        }

        editor.last_requested_preview_time = Some(timestamp_secs);
        editor.preview_frame_in_flight = true;
        editor.pending_preview_request = None;
        editor.playback.request_preview_frame(timestamp_secs);
    }

    fn queue_fast_preview_request(&mut self, timestamp_secs: f64) {
        let Some(editor) = self.editor.as_mut() else {
            return;
        };

        let timestamp_secs = timestamp_secs.clamp(0.0, editor.duration_secs());

        // Skip if decoder is already processing a request
        if editor.playback.is_frame_request_in_flight() {
            return;
        }

        // More aggressive debouncing during scrubbing (150ms instead of 100ms)
        // This prevents overwhelming the decoder with requests during fast scrub
        if let Some(last_requested) = editor.last_requested_preview_time {
            if editor.preview_texture.is_some() && (last_requested - timestamp_secs).abs() < 0.15 {
                return;
            }
        }

        editor.last_requested_preview_time = Some(timestamp_secs);
        editor.playback.request_preview_frame_fast(timestamp_secs);
    }

    fn poll_background_work(&mut self, ctx: &egui::Context) -> Option<f64> {
        let mut follow_up_preview = None;

        while let Ok(result) = self.thumbnail_rx.try_recv() {
            self.thumbnails_generating.remove(&result.video_path);
            if let Some(image) = result.image {
                let texture_name = result
                    .video_path
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_else(|| "thumbnail".to_string());
                self.insert_thumbnail_texture(ctx, result.video_path, &texture_name, image);
            } else if let Some(error) = result.error {
                warn!("Thumbnail generation failed: {error}");
            }
        }

        if let Some(editor) = self.editor.as_mut() {
            let wall_time = if editor.is_playing && !editor.preview_final {
                Some(editor.playback.playback_position_secs())
            } else {
                None
            };
            editor.playback.poll_with_wall_time(wall_time);

            // Live playback path: drain the frame queue for frames due by now.
            if editor.is_playing && !editor.preview_final {
                let wall_time = wall_time.unwrap();
                let (queue_len, _) = editor.playback.cache_stats();
                if let Some(image) = editor.playback.take_playback_frame(wall_time) {
                    tracing::trace!(
                        "poll_background_work: displaying frame at wall_time={:.3}s",
                        wall_time
                    );
                    let color_image = color_image_from_rgba(&image);
                    if let Some(texture) = &mut editor.preview_texture {
                        texture.set(color_image, egui::TextureOptions::LINEAR);
                    } else {
                        editor.preview_texture = Some(ctx.load_texture(
                            format!("preview:{}", editor.video.filename),
                            color_image,
                            egui::TextureOptions::LINEAR,
                        ));
                    }
                    editor.preview_frame_in_flight = false;
                    editor.pending_preview_request = None;
                    editor.error_message = None;
                } else if queue_len > 0 {
                    tracing::trace!(
                        "poll_background_work: no frame for wall_time={:.3}s, queue_len={}",
                        wall_time,
                        queue_len
                    );
                }
            }

            // Static / single-frame preview path (paused or preview_final mode).
            if let Some(frame) = editor.playback.take_frame() {
                let color_image = color_image_from_rgba(&frame.image);
                if let Some(texture) = &mut editor.preview_texture {
                    texture.set(color_image, egui::TextureOptions::LINEAR);
                } else {
                    editor.preview_texture = Some(ctx.load_texture(
                        format!("preview:{}", editor.video.filename),
                        color_image,
                        egui::TextureOptions::LINEAR,
                    ));
                }
                editor.preview_frame_in_flight = false;
                editor.error_message = None;
                if let Some(pending) = editor.pending_preview_request.take() {
                    follow_up_preview = Some(pending);
                } else if editor.is_playing {
                    follow_up_preview = None;
                }
            }
            if let Some(error) = editor.playback.take_error() {
                editor.preview_frame_in_flight = false;
                editor.error_message = Some(error);
                if let Some(pending) = editor.pending_preview_request.take() {
                    follow_up_preview = Some(pending);
                }
            }
        }

        follow_up_preview
    }

    pub fn update(&mut self, ctx: &egui::Context, _is_open: &mut bool) {
        let mut requested_preview = self.poll_background_work(ctx);

        if !self.loaded {
            self.scan_videos(ctx);
        }

        let mut browser_outcome = BrowserUiOutcome::default();
        let mut editor_outcome = EditorUiOutcome::default();

        egui::CentralPanel::default().show(ctx, |ui| {
            if self.editor.is_some() {
                editor_outcome = self.render_editor(ui);
            } else {
                browser_outcome = self.render_browser(ui);
            }
        });

        for video in browser_outcome.thumbnails_to_generate {
            self.schedule_thumbnail_generation(&video);
        }

        if browser_outcome.refresh_requested || editor_outcome.refresh_browser {
            self.refresh();
        }

        if let Some(video) = browser_outcome.selected_video {
            self.open_editor(video);
            requested_preview = Some(0.0);
        }

        if editor_outcome.back_to_browser {
            self.editor = None;
        }

        if let Some(preview_request) = editor_outcome.preview_request {
            requested_preview = Some(preview_request);
        }

        if let Some(fast_preview_request) = editor_outcome.fast_preview_request {
            self.queue_fast_preview_request(fast_preview_request);
        }

        if let Some(preview_request) = requested_preview {
            self.queue_preview_request(preview_request);
        }

        if self.should_repaint() {
            let repaint_ms = self
                .editor
                .as_ref()
                .filter(|e| e.is_playing && !e.preview_final)
                .map(|e| (1000.0 / e.playback.playback_fps()).clamp(8.0, 50.0) as u64)
                .unwrap_or(80);
            ctx.request_repaint_after(Duration::from_millis(repaint_ms));
        }
    }

    fn should_repaint(&self) -> bool {
        if !self.thumbnails_generating.is_empty() {
            return true;
        }
        let Some(editor) = self.editor.as_ref() else {
            return false;
        };
        editor.is_playing || editor.playback.has_pending_activity() || editor.has_active_export()
    }

    fn open_editor(&mut self, video: VideoEntry) {
        info!("Opening Clip & Compress editor for {:?}", video.path);
        self.editor = Some(EditorState::new(video));
    }

    fn render_browser(&mut self, ui: &mut egui::Ui) -> BrowserUiOutcome {
        browser::render_browser_ui(self, ui)
    }

    fn render_editor(&mut self, ui: &mut egui::Ui) -> EditorUiOutcome {
        editor::render_editor_ui(self, ui)
    }

    pub fn refresh(&mut self) {
        self.loaded = false;
        self.scan_error = None;
        self.videos_by_game.clear();
        self.thumbnails.clear();
        self.thumbnails_generating.clear();
    }
}

#[derive(Default)]
struct BrowserUiOutcome {
    thumbnails_to_generate: Vec<VideoEntry>,
    selected_video: Option<VideoEntry>,
    refresh_requested: bool,
}

#[derive(Default)]
struct EditorUiOutcome {
    preview_request: Option<f64>,
    fast_preview_request: Option<f64>,
    back_to_browser: bool,
    refresh_browser: bool,
}

fn collect_video_paths(dir: &Path, cache_dir: &Path, output: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path == cache_dir {
            continue;
        }
        if path.is_dir() {
            collect_video_paths(&path, cache_dir, output);
        } else if path
            .extension()
            .map(|ext| ext.eq_ignore_ascii_case("mp4"))
            .unwrap_or(false)
        {
            output.push(path);
        }
    }
}

fn render_preview_panel(
    ui: &mut egui::Ui,
    editor: &mut EditorState,
    outcome: &mut EditorUiOutcome,
) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        let available_width = ui.available_width().max(220.0);
        let aspect_ratio = (editor.video.metadata.width.max(1) as f32
            / editor.video.metadata.height.max(1) as f32)
            .max(1.0 / 3.0);
        let available_height = ui.available_height().max(220.0);
        let mut preview_height = (available_width / aspect_ratio).max(180.0);
        let max_preview_height = (available_height - 72.0).max(180.0);
        preview_height = preview_height.min(max_preview_height);
        let preview_size = egui::vec2(available_width, preview_height);
        if let Some(texture) = &editor.preview_texture {
            ui.add(
                egui::Image::from_texture(texture)
                    .fit_to_exact_size(preview_size)
                    .maintain_aspect_ratio(true),
            );
        } else {
            let (rect, _) = ui.allocate_exact_size(preview_size, egui::Sense::hover());
            ui.painter().rect_filled(
                rect,
                egui::CornerRadius::same(6),
                egui::Color32::from_rgb(18, 20, 24),
            );
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Loading embedded preview...",
                egui::FontId::proportional(18.0),
                egui::Color32::from_rgb(160, 165, 175),
            );
            if !editor.preview_frame_in_flight {
                outcome.preview_request = Some(editor.current_time_secs);
            }
        }

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let label = if editor.is_playing { "Pause" } else { "Play" };
            if ui.button(label).clicked() {
                if editor.is_playing {
                    editor.playback.pause_at(editor.current_time_secs);
                    editor.is_playing = false;
                } else {
                    editor.last_tick = Instant::now();
                    editor.is_playing = true;
                    if editor.preview_final {
                        editor.current_time_secs = clamp_to_enabled_playback_time(
                            editor.current_time_secs,
                            editor.duration_secs(),
                            &editor.cut_points,
                            &editor.snippet_enabled,
                        );
                        outcome.preview_request = Some(editor.current_time_secs);
                    } else {
                        editor.preview_frame_in_flight = false;
                        editor.pending_preview_request = None;
                        editor.playback.play_from(editor.current_time_secs);
                    }
                }
            }
            ui.label(format!(
                "Current: {}",
                format_timestamp_precise(editor.current_time_secs)
            ));
            if editor.preview_final {
                ui.label(egui::RichText::new("Previewing kept ranges only").strong());
            }
        });

        let duration = editor.duration_secs();
        let response = ui.add(
            egui::Slider::new(&mut editor.current_time_secs, 0.0..=duration).show_value(false),
        );
        if response.changed() {
            editor.playback.pause_at(editor.current_time_secs);
            editor.is_playing = false;
            editor.current_time_secs = editor.current_time_secs.clamp(0.0, editor.duration_secs());
            outcome.preview_request = Some(editor.current_time_secs);
        }
    });
}

fn render_editor_stats(ui: &mut egui::Ui, editor: &EditorState) {
    ui.horizontal_wrapped(|ui| {
        ui.label(format!(
            "Duration: {}",
            format_compact_duration(editor.duration_secs())
        ));
        ui.separator();
        ui.label(format!(
            "Original Size: {}",
            format_size_mb(editor.video.size_mb)
        ));
        ui.separator();
        ui.label(format!(
            "Output Duration: {}",
            format_compact_duration(editor.kept_duration_secs())
        ));
        ui.separator();
        ui.label(format!(
            "Resolution: {}x{}",
            editor.video.metadata.width, editor.video.metadata.height
        ));
        if editor.video.metadata.has_audio {
            ui.separator();
            ui.label("Audio: included");
        }
        ui.separator();
        let (cache_entries, cache_mb) = editor.playback.cache_stats();
        ui.label(format!(
            "Cache: {} frames ({:.1} MB)",
            cache_entries, cache_mb
        ));
    });
}

fn render_timeline_panel(
    ui: &mut egui::Ui,
    editor: &mut EditorState,
    outcome: &mut EditorUiOutcome,
) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.label(egui::RichText::new("Timeline").strong());
        ui.add_space(4.0);
        render_timeline(ui, editor, outcome);
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button("Add Cut at Playhead").clicked()
                && add_cut_point(editor, editor.current_time_secs)
            {
                outcome.preview_request = Some(editor.current_time_secs);
            }
            if ui
                .add_enabled(
                    editor.selected_cut_point.is_some(),
                    egui::Button::new("Remove Selected Cut"),
                )
                .clicked()
            {
                if let Some(index) = editor.selected_cut_point {
                    remove_cut_point(editor, index);
                    outcome.preview_request = Some(editor.current_time_secs);
                }
            }
            if ui.button("Clear All Splits").clicked() {
                clear_cut_points(editor);
                outcome.preview_request = Some(editor.current_time_secs);
            }
        });
    });
}

fn render_timeline(ui: &mut egui::Ui, editor: &mut EditorState, outcome: &mut EditorUiOutcome) {
    let timeline_height = ui.available_height().clamp(92.0, 180.0);
    let desired_size = egui::vec2(ui.available_width(), timeline_height);
    let (rect, response) = ui.allocate_exact_size(desired_size, egui::Sense::click_and_drag());
    let track_rect = egui::Rect::from_min_max(
        rect.min + egui::vec2(0.0, 12.0),
        rect.max - egui::vec2(0.0, 22.0),
    );
    let painter = ui.painter();
    for snippet in editor.snippets() {
        let left = time_to_x(track_rect, snippet.start_secs, editor.duration_secs());
        let right = time_to_x(track_rect, snippet.end_secs, editor.duration_secs()).max(left + 2.0);
        let snippet_rect = egui::Rect::from_min_max(
            egui::pos2(left, track_rect.top()),
            egui::pos2(right, track_rect.bottom()),
        );
        let color = if snippet.enabled {
            egui::Color32::from_rgb(48, 92, 156)
        } else {
            egui::Color32::from_rgb(96, 44, 44)
        };
        painter.rect_filled(snippet_rect, egui::CornerRadius::same(6), color);
    }

    for (index, cut_point) in editor.cut_points.iter().enumerate() {
        let x = time_to_x(track_rect, *cut_point, editor.duration_secs());
        let color = if editor.selected_cut_point == Some(index) {
            egui::Color32::WHITE
        } else {
            egui::Color32::from_rgb(220, 220, 220)
        };
        painter.line_segment(
            [
                egui::pos2(x, track_rect.top()),
                egui::pos2(x, track_rect.bottom()),
            ],
            egui::Stroke::new(2.0, color),
        );
    }

    let playhead_x = time_to_x(track_rect, editor.current_time_secs, editor.duration_secs());
    painter.line_segment(
        [
            egui::pos2(playhead_x, track_rect.top() - 4.0),
            egui::pos2(playhead_x, track_rect.bottom() + 4.0),
        ],
        egui::Stroke::new(2.0, egui::Color32::from_rgb(236, 201, 75)),
    );

    for ratio in [0.0_f32, 0.25, 0.5, 0.75, 1.0] {
        let time = editor.duration_secs() * f64::from(ratio);
        let x = egui::lerp(track_rect.left()..=track_rect.right(), ratio);
        painter.text(
            egui::pos2(x, rect.bottom() - 2.0),
            egui::Align2::CENTER_BOTTOM,
            format_compact_duration(time),
            egui::FontId::proportional(11.0),
            egui::Color32::from_rgb(160, 165, 175),
        );
    }

    if let Some(pointer) = response.interact_pointer_pos() {
        if response.clicked() || response.dragged() {
            if let Some(index) = hit_test_cut_point(editor, track_rect, pointer.x) {
                editor.selected_cut_point = Some(index);
            } else {
                editor.selected_cut_point = None;
                let new_time_secs = x_to_time(track_rect, pointer.x, editor.duration_secs());

                // Calculate scrub speed for adaptive resolution
                let now = Instant::now();
                if let (Some(last_time), Some(last_pos)) =
                    (editor.last_scrub_time, editor.last_scrub_position)
                {
                    let dt = last_time.elapsed().as_secs_f64();
                    let dx = (new_time_secs - last_pos).abs();
                    if dt > 0.001 {
                        let speed = dx / dt; // seconds of video per second of wall time
                        let fps = editor.video.metadata.fps;

                        // Update scrub state based on speed relative to playback
                        editor.scrub_state = if speed > fps * 2.0 {
                            ScrubState::DraggingFast
                        } else if speed > fps * 0.5 {
                            ScrubState::DraggingNormal
                        } else {
                            ScrubState::DraggingSlow
                        }
                    }
                }
                editor.last_scrub_time = Some(now);
                editor.last_scrub_position = Some(new_time_secs);
                editor.current_time_secs = new_time_secs;

                if editor.is_playing {
                    // When playing, just scrub to the new position
                    // The decoder will seek and continue producing frames
                    editor.playback.scrub_to(editor.current_time_secs);

                    // When drag stops, commit_scrub will sync the final position
                } else {
                    // When paused, pause at position and request preview frame
                    editor.playback.pause_at(editor.current_time_secs);
                    if response.dragged() {
                        outcome.fast_preview_request = Some(editor.current_time_secs);
                    } else {
                        outcome.preview_request = Some(editor.current_time_secs);
                    }
                }
            }
        }
    }

    if response.drag_stopped() && editor.is_playing {
        editor.playback.commit_scrub(editor.current_time_secs);
        editor.scrub_state = ScrubState::Idle;
        editor.last_scrub_time = None;
        editor.last_scrub_position = None;
    }
}

fn render_editor_workspace(
    ui: &mut egui::Ui,
    editor: &mut EditorState,
    outcome: &mut EditorUiOutcome,
) {
    let available_size = ui.available_size();
    let stacked_layout = available_size.x < EDITOR_STACK_BREAKPOINT;

    if stacked_layout {
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                render_editor_main_panel(ui, editor, outcome);
                ui.add_space(12.0);
                render_editor_sidebar(ui, editor, outcome, false);
            });
        return;
    }

    ui.horizontal_top(|ui| {
        let sidebar_width =
            (available_size.x * 0.32).clamp(EDITOR_SIDEBAR_MIN_WIDTH, EDITOR_SIDEBAR_WIDTH);
        let main_width = (ui.available_width() - sidebar_width - 12.0).max(320.0);

        ui.allocate_ui_with_layout(
            egui::vec2(main_width, available_size.y.max(320.0)),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                render_editor_main_panel(ui, editor, outcome);
            },
        );

        ui.add_space(12.0);

        ui.allocate_ui_with_layout(
            egui::vec2(sidebar_width, available_size.y.max(320.0)),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                render_editor_sidebar(ui, editor, outcome, true);
            },
        );
    });
}

fn render_editor_main_panel(
    ui: &mut egui::Ui,
    editor: &mut EditorState,
    outcome: &mut EditorUiOutcome,
) {
    render_preview_panel(ui, editor, outcome);
    ui.add_space(10.0);
    egui::Frame::group(ui.style()).show(ui, |ui| {
        render_editor_stats(ui, editor);
    });
    ui.add_space(10.0);
    render_timeline_panel(ui, editor, outcome);
}

fn render_editor_sidebar(
    ui: &mut egui::Ui,
    editor: &mut EditorState,
    outcome: &mut EditorUiOutcome,
    fill_height: bool,
) {
    let render_contents =
        |ui: &mut egui::Ui, editor: &mut EditorState, outcome: &mut EditorUiOutcome| {
            render_snippet_list(ui, editor, outcome);
            ui.add_space(10.0);
            render_size_section(ui, editor);
            ui.add_space(10.0);
            render_action_section(ui, editor, outcome);
        };

    if fill_height {
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                render_contents(ui, editor, outcome);
            });
    } else {
        render_contents(ui, editor, outcome);
    }
}

fn render_action_section(
    ui: &mut egui::Ui,
    editor: &mut EditorState,
    outcome: &mut EditorUiOutcome,
) {
    let can_export = !editor.kept_ranges().is_empty() && editor.target_size_mb > 0;

    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.label(egui::RichText::new("Actions").strong());
        ui.add_space(6.0);

        let preview_label = if editor.preview_final {
            "Stop Preview"
        } else {
            "Preview Final"
        };

        if ui
            .add_sized(
                [ui.available_width(), 32.0],
                egui::Button::new(preview_label),
            )
            .clicked()
        {
            editor.playback.pause_at(editor.current_time_secs);
            editor.preview_final = !editor.preview_final;
            editor.is_playing = editor.preview_final;
            editor.last_tick = Instant::now();
            if editor.preview_final {
                editor.current_time_secs = clamp_to_enabled_playback_time(
                    editor.current_time_secs,
                    editor.duration_secs(),
                    &editor.cut_points,
                    &editor.snippet_enabled,
                );
            }
            outcome.preview_request = Some(editor.current_time_secs);
        }

        ui.add_space(6.0);

        if ui
            .add_enabled(can_export, egui::Button::new("Export Clip"))
            .clicked()
        {
            start_export(editor);
        }

        if editor.preview_final {
            ui.add_space(8.0);
            ui.label(egui::RichText::new("Previewing kept ranges only").strong());
        }
    });
}

fn hit_test_cut_point(editor: &EditorState, rect: egui::Rect, pointer_x: f32) -> Option<usize> {
    let mut best_match = None;
    let mut best_distance = f32::MAX;

    for (index, cut_point) in editor.cut_points.iter().enumerate() {
        let x = time_to_x(rect, *cut_point, editor.duration_secs());
        let distance = (pointer_x - x).abs();
        if distance < 8.0 && distance < best_distance {
            best_distance = distance;
            best_match = Some(index);
        }
    }

    best_match
}

fn render_snippet_list(ui: &mut egui::Ui, editor: &mut EditorState, outcome: &mut EditorUiOutcome) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.label(egui::RichText::new("Snippets").strong());
        ui.label(egui::RichText::new("Use the timeline and add cuts at the playhead to split the clip into snippets. Disabled snippets are skipped in preview/export.").weak());

        let snippets = editor.snippets();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .max_height(260.0)
            .show(ui, |ui| {
                for (index, snippet) in snippets.iter().copied().enumerate() {
                    egui::Frame::group(ui.style())
                        .inner_margin(egui::Margin::same(8))
                        .show(ui, |ui| {
                            ui.horizontal_wrapped(|ui| {
                                let mut enabled = editor.snippet_enabled.get(index).copied().unwrap_or(true);
                                if ui.checkbox(&mut enabled, format!("Snippet {}", index + 1)).changed() {
                                    if let Some(flag) = editor.snippet_enabled.get_mut(index) {
                                        *flag = enabled;
                                    }
                                    outcome.preview_request = Some(editor.current_time_secs);
                                }
                                ui.label(format!(
                                    "{} to {} ({})",
                                    format_timestamp_precise(snippet.start_secs),
                                    format_timestamp_precise(snippet.end_secs),
                                    format_compact_duration(snippet.duration_secs()),
                                ));
                                if index < editor.cut_points.len() && ui.button("Remove following cut").clicked() {
                                    remove_cut_point(editor, index);
                                    outcome.preview_request = Some(editor.current_time_secs);
                                }
                            });
                        });
                    ui.add_space(6.0);
                }
            });
    });
}

fn render_size_section(ui: &mut egui::Ui, editor: &mut EditorState) {
    let kept_duration = editor.kept_duration_secs();
    let (video_kbps, total_kbps) = estimate_bitrate_kbps(
        editor.target_size_mb,
        kept_duration,
        editor.video.metadata.has_audio,
    );
    let (quality_label, bars) = quality_estimate(&editor.video.metadata, video_kbps);

    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.label(egui::RichText::new("Export Settings").strong());
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            ui.label("Target Output Size:");
            ui.add(
                egui::DragValue::new(&mut editor.target_size_mb)
                    .range(1..=4096)
                    .suffix(" MB")
                    .speed(1),
            );
        });
        ui.label(format!(
            "Estimated Quality: [{}{}] {} (video ~{:.2} Mbps, total ~{:.2} Mbps)",
            "#".repeat(bars),
            "-".repeat(5 - bars),
            quality_label,
            video_kbps as f64 / 1000.0,
            total_kbps as f64 / 1000.0,
        ));
        ui.label(format!(
            "Kept duration after cuts: {}",
            format_compact_duration(kept_duration)
        ));
    });
}

fn update_playback_clock(editor: &mut EditorState, outcome: &mut EditorUiOutcome) {
    if !editor.is_playing || editor.has_active_export() {
        editor.last_tick = Instant::now();
        return;
    }

    if !editor.preview_final {
        editor.current_time_secs = editor.playback.playback_position_secs();
        if editor.current_time_secs >= editor.duration_secs() {
            editor.current_time_secs = editor.duration_secs();
            editor.is_playing = false;
            editor.playback.pause_at(editor.current_time_secs);
        }
        return;
    }

    let now = Instant::now();
    let delta = now.duration_since(editor.last_tick).as_secs_f64();
    editor.last_tick = now;
    if delta <= 0.0 {
        return;
    }

    editor.current_time_secs += delta;
    if editor.preview_final {
        editor.current_time_secs = clamp_to_enabled_playback_time(
            editor.current_time_secs,
            editor.duration_secs(),
            &editor.cut_points,
            &editor.snippet_enabled,
        );
    }

    if editor.current_time_secs >= editor.duration_secs() {
        editor.current_time_secs = editor.duration_secs();
        editor.is_playing = false;
        editor.preview_final = false;
    }

    if editor.preview_frame_in_flight {
        editor.pending_preview_request = Some(editor.current_time_secs);
    } else if editor
        .last_requested_preview_time
        .map(|last| (editor.current_time_secs - last).abs() >= PREVIEW_FRAME_INTERVAL_SECS)
        .unwrap_or(true)
    {
        outcome.preview_request = Some(editor.current_time_secs);
    }
}

fn start_export(editor: &mut EditorState) {
    let kept_ranges = editor.kept_ranges();
    if kept_ranges.is_empty() {
        editor.error_message =
            Some("At least one snippet must stay enabled for export".to_string());
        return;
    }

    let output_path = build_clipped_output_path(&editor.video);
    let (progress_tx, progress_rx) = mpsc::channel();
    let cancel_flag = Arc::new(AtomicBool::new(false));

    spawn_clip_export(
        ClipExportRequest {
            input_path: editor.video.path.clone(),
            output_path,
            keep_ranges: kept_ranges,
            target_size_mb: editor.target_size_mb,
            audio_bitrate_kbps: DEFAULT_AUDIO_BITRATE_KBPS,
            metadata: editor.video.metadata.clone(),
        },
        progress_tx,
        cancel_flag.clone(),
    );

    editor.export_state = Some(ExportState {
        progress_rx,
        cancel_flag,
        progress: 0.0,
        message: "Preparing export".to_string(),
    });
    editor.export_output = None;
    editor.status_message = None;
    editor.error_message = None;
    editor.playback.pause_at(editor.current_time_secs);
    editor.is_playing = false;
    editor.preview_final = false;
}

fn poll_editor_export_updates(editor: &mut EditorState, outcome: &mut EditorUiOutcome) {
    let mut finished_path = None;
    let mut failed_message = None;
    let mut cancelled = false;

    if let Some(export) = editor.export_state.as_mut() {
        while let Ok(update) = export.progress_rx.try_recv() {
            match update {
                ClipExportUpdate::Progress {
                    phase: _,
                    fraction,
                    message,
                } => {
                    export.progress = fraction;
                    export.message = message;
                }
                ClipExportUpdate::Finished(path) => {
                    finished_path = Some(path);
                }
                ClipExportUpdate::Failed(message) => {
                    failed_message = Some(message);
                }
                ClipExportUpdate::Cancelled => {
                    cancelled = true;
                }
            }
        }
    }

    if let Some(path) = finished_path {
        editor.export_state = None;
        editor.export_output = Some(path);
        editor.status_message = Some("Export complete".to_string());
        editor.error_message = None;
        outcome.refresh_browser = true;
        show_toast(ToastKind::Success, "Clip export completed");
    } else if let Some(message) = failed_message {
        editor.export_state = None;
        editor.error_message = Some(message);
        show_toast(ToastKind::Error, "Clip export failed");
    } else if cancelled {
        editor.export_state = None;
        editor.status_message = Some("Export cancelled".to_string());
        show_toast(ToastKind::Warning, "Clip export cancelled");
    }
}

fn render_completion_screen(ui: &mut egui::Ui, editor: &mut EditorState) -> EditorUiOutcome {
    let mut outcome = EditorUiOutcome::default();
    let output_path = editor.export_output.clone().unwrap();

    ui.horizontal(|ui| {
        if ui.button("< Back to Videos").clicked() {
            outcome.back_to_browser = true;
            outcome.refresh_browser = true;
        }
        ui.heading("Export Complete");
    });
    ui.separator();

    ui.vertical_centered(|ui| {
        ui.add_space(50.0);
        ui.label(
            egui::RichText::new("Clip exported successfully")
                .size(20.0)
                .strong(),
        );
        ui.label(output_path.display().to_string());
        ui.add_space(14.0);
        if ui.button("Open Output Folder").clicked() {
            if let Err(err) = open_path(output_path.parent().unwrap_or(&output_path)) {
                editor.error_message = Some(format!("Failed to open folder: {err:#}"));
            }
        }
        if ui.button("Play New Clip").clicked() {
            if let Err(err) = open_path(&output_path) {
                editor.error_message = Some(format!("Failed to open clip: {err:#}"));
            }
        }
        if ui.button("Return to Browser").clicked() {
            outcome.back_to_browser = true;
            outcome.refresh_browser = true;
        }
    });

    outcome
}

fn open_path(path: &Path) -> anyhow::Result<()> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        Command::new("cmd")
            .args(["/C", "start", "", &path.to_string_lossy()])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(anyhow::Error::from)?;
        Ok(())
    }

    #[cfg(not(target_os = "windows"))]
    {
        Command::new("xdg-open")
            .arg(path)
            .spawn()
            .map_err(anyhow::Error::from)?;
        Ok(())
    }
}

fn build_clipped_output_path(video: &VideoEntry) -> PathBuf {
    let game_folder = format!("Clipped-{}", video.game.replace(['\\', '/'], "-"));
    let output_dir = video.save_root.join(game_folder);
    let _ = std::fs::create_dir_all(&output_dir);

    let stem = video
        .path
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or_else(|| "clip".to_string());

    for attempt in 0..1000 {
        let suffix = if attempt == 0 {
            "_clipped".to_string()
        } else {
            format!("_clipped_{attempt}")
        };
        let candidate = output_dir.join(format!("{stem}{suffix}.mp4"));
        if !candidate.exists() {
            return candidate;
        }
    }

    output_dir.join(format!(
        "{}_clipped_{}.mp4",
        stem,
        chrono::Local::now().format("%Y%m%d_%H%M%S")
    ))
}

fn load_rgba_image_from_path(path: &Path) -> anyhow::Result<RgbaImage> {
    Ok(image::open(path)?.into_rgba8())
}

fn color_image_from_rgba(image: &RgbaImage) -> egui::ColorImage {
    egui::ColorImage::from_rgba_unmultiplied(
        [image.width() as usize, image.height() as usize],
        image.as_raw(),
    )
}

fn format_size_mb(size_mb: f64) -> String {
    if size_mb >= 100.0 {
        format!("{size_mb:.0} MB")
    } else {
        format!("{size_mb:.1} MB")
    }
}

fn format_compact_duration(seconds: f64) -> String {
    let total_seconds = seconds.max(0.0).round() as u64;
    let hours = total_seconds / 3600;
    let minutes = (total_seconds / 60) % 60;
    let secs = total_seconds % 60;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{secs:02}")
    } else {
        format!("{minutes}:{secs:02}")
    }
}

fn format_timestamp_precise(seconds: f64) -> String {
    let total_millis = (seconds.max(0.0) * 1000.0).round() as u64;
    let hours = total_millis / 3_600_000;
    let minutes = (total_millis / 60_000) % 60;
    let secs = (total_millis / 1000) % 60;
    let millis = total_millis % 1000;
    format!("{hours:02}:{minutes:02}:{secs:02}.{millis:03}")
}

fn normalize_cut_points(cut_points: &mut Vec<f64>, duration_secs: f64) {
    cut_points.retain(|point| *point > MIN_RANGE_SECS && *point < duration_secs - MIN_RANGE_SECS);
    cut_points.sort_by(|a, b| a.total_cmp(b));
    cut_points.dedup_by(|a, b| (*a - *b).abs() < MIN_RANGE_SECS);
}

fn clear_cut_points(editor: &mut EditorState) {
    editor.cut_points.clear();
    editor.snippet_enabled.clear();
    editor.snippet_enabled.push(true);
    editor.selected_cut_point = None;
}

fn add_cut_point(editor: &mut EditorState, time_secs: f64) -> bool {
    let duration = editor.duration_secs();
    let cut_time = time_secs.clamp(0.0, duration);
    if cut_time <= MIN_RANGE_SECS || cut_time >= duration - MIN_RANGE_SECS {
        editor.error_message = Some("Cuts must stay inside the clip boundaries".to_string());
        return false;
    }

    let insert_index = match editor
        .cut_points
        .binary_search_by(|probe| probe.total_cmp(&cut_time))
    {
        Ok(_) => {
            editor.error_message = Some("A cut already exists near that point".to_string());
            return false;
        }
        Err(index) => index,
    };

    let previous_boundary = if insert_index == 0 {
        0.0
    } else {
        editor.cut_points[insert_index - 1]
    };
    let next_boundary = editor
        .cut_points
        .get(insert_index)
        .copied()
        .unwrap_or(duration);
    if cut_time - previous_boundary < MIN_RANGE_SECS || next_boundary - cut_time < MIN_RANGE_SECS {
        editor.error_message = Some("Cuts must leave each snippet with some duration".to_string());
        return false;
    }

    let inherited = editor
        .snippet_enabled
        .get(insert_index)
        .copied()
        .unwrap_or(true);
    editor.cut_points.insert(insert_index, cut_time);
    editor.snippet_enabled.insert(insert_index + 1, inherited);
    normalize_cut_points(&mut editor.cut_points, duration);
    editor.selected_cut_point = Some(insert_index);
    editor.error_message = None;
    true
}

fn remove_cut_point(editor: &mut EditorState, index: usize) {
    if index >= editor.cut_points.len() {
        return;
    }

    editor.cut_points.remove(index);
    let right_enabled = if index + 1 < editor.snippet_enabled.len() {
        editor.snippet_enabled.remove(index + 1)
    } else {
        true
    };
    if let Some(left_enabled) = editor.snippet_enabled.get_mut(index) {
        *left_enabled = *left_enabled || right_enabled;
    }
    editor.selected_cut_point = index
        .checked_sub(1)
        .or(Some(index).filter(|i| *i < editor.cut_points.len()));
    editor.error_message = None;
}

fn snippet_segments(
    duration_secs: f64,
    cut_points: &[f64],
    snippet_enabled: &[bool],
) -> Vec<SnippetSegment> {
    let mut segments = Vec::with_capacity(cut_points.len() + 1);
    let mut start_secs = 0.0;

    for (index, end_secs) in cut_points
        .iter()
        .copied()
        .chain(std::iter::once(duration_secs))
        .enumerate()
    {
        let enabled = snippet_enabled.get(index).copied().unwrap_or(true);
        segments.push(SnippetSegment {
            start_secs,
            end_secs,
            enabled,
        });
        start_secs = end_secs;
    }

    segments
}

fn enabled_time_ranges(
    duration_secs: f64,
    cut_points: &[f64],
    snippet_enabled: &[bool],
) -> Vec<TimeRange> {
    snippet_segments(duration_secs, cut_points, snippet_enabled)
        .into_iter()
        .filter(|segment| segment.enabled && segment.duration_secs() >= MIN_RANGE_SECS)
        .map(|segment| TimeRange {
            start_secs: segment.start_secs,
            end_secs: segment.end_secs,
        })
        .collect()
}

fn clamp_to_enabled_playback_time(
    time_secs: f64,
    duration_secs: f64,
    cut_points: &[f64],
    snippet_enabled: &[bool],
) -> f64 {
    let mut next_enabled_start = duration_secs;

    for snippet in snippet_segments(duration_secs, cut_points, snippet_enabled) {
        if snippet.enabled {
            if time_secs < snippet.start_secs {
                return snippet.start_secs;
            }
            if time_secs >= snippet.start_secs && time_secs < snippet.end_secs {
                return time_secs;
            }
            next_enabled_start = duration_secs;
        } else if time_secs >= snippet.start_secs && time_secs < snippet.end_secs {
            next_enabled_start = snippet.end_secs;
        }
    }

    next_enabled_start.min(duration_secs)
}

fn estimate_bitrate_kbps(
    target_size_mb: u32,
    kept_duration_secs: f64,
    has_audio: bool,
) -> (u32, u32) {
    let total_kbps = (f64::from(target_size_mb) * 8192.0 / kept_duration_secs.max(1.0)).max(256.0);
    let audio_kbps = if has_audio {
        DEFAULT_AUDIO_BITRATE_KBPS
    } else {
        0
    };
    let video_kbps = (total_kbps - f64::from(audio_kbps) - 24.0)
        .max(300.0)
        .round() as u32;
    (video_kbps, total_kbps.round() as u32)
}

fn quality_estimate(metadata: &VideoFileMetadata, video_kbps: u32) -> (&'static str, usize) {
    let pixel_factor =
        ((metadata.width as f64 * metadata.height as f64) / (1920.0 * 1080.0)).clamp(0.35, 2.0);
    let medium_threshold = 2000.0 * pixel_factor;
    let high_threshold = 5000.0 * pixel_factor;
    let bitrate = video_kbps as f64;
    if bitrate >= high_threshold {
        ("High", 5)
    } else if bitrate >= medium_threshold {
        ("Medium", 3)
    } else {
        ("Low", 2)
    }
}

fn time_to_x(rect: egui::Rect, time_secs: f64, duration_secs: f64) -> f32 {
    let ratio = if duration_secs <= 0.0 {
        0.0
    } else {
        (time_secs / duration_secs).clamp(0.0, 1.0) as f32
    };
    egui::lerp(rect.left()..=rect.right(), ratio)
}

fn x_to_time(rect: egui::Rect, x: f32, duration_secs: f64) -> f64 {
    if rect.width() <= 0.0 || duration_secs <= 0.0 {
        return 0.0;
    }
    let ratio = ((x - rect.left()) / rect.width()).clamp(0.0, 1.0);
    duration_secs * f64::from(ratio)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snippet_segments_follow_cut_points() {
        let snippets = snippet_segments(30.0, &[5.0, 20.0], &[true, false, true]);
        assert_eq!(snippets.len(), 3);
        assert!((snippets[0].duration_secs() - 5.0).abs() < 0.001);
        assert!(!snippets[1].enabled);
        assert!((snippets[2].start_secs - 20.0).abs() < 0.001);
    }

    #[test]
    fn enabled_ranges_skip_disabled_snippets() {
        let kept = enabled_time_ranges(30.0, &[5.0, 20.0], &[true, false, true]);
        assert_eq!(kept.len(), 2);
        assert!((kept[0].duration_secs() - 5.0).abs() < 0.001);
        assert!((kept[1].start_secs - 20.0).abs() < 0.001);
    }

    #[test]
    fn playback_clamps_to_next_enabled_snippet() {
        let next_time =
            clamp_to_enabled_playback_time(7.5, 30.0, &[5.0, 20.0], &[true, false, true]);
        assert!((next_time - 20.0).abs() < 0.001);
    }
}
