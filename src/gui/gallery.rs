use anyhow::Context;
use eframe::egui;
use image::RgbaImage;
use rayon::prelude::*;
use rustc_hash::FxHashMap;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use tokio::sync::mpsc::Sender as TokioSender;
use tracing::{error, info, warn};

mod browser;
mod decode_pipeline;
mod editor;
mod editor_panels;
mod utils;

use decode_pipeline::PlaybackController;

use crate::config::{Config, EncoderType};
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
const SCRUB_SAMPLE_MIN_DT_SECS: f64 = 0.01;
const SCRUB_FAST_RATE_SECS_PER_SEC: f64 = 6.0;
const MIN_RANGE_SECS: f64 = 0.1;
const EDITOR_SIDEBAR_WIDTH: f32 = 340.0;
const EDITOR_SIDEBAR_MIN_WIDTH: f32 = 280.0;
const EDITOR_STACK_BREAKPOINT: f32 = 960.0;
const EDITOR_SMALL_SEEK_SECS: f64 = 1.0;
const EDITOR_LARGE_SEEK_SECS: f64 = 5.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardSize {
    Small,
    Medium,
    Large,
    XLarge,
}

impl CardSize {
    pub fn dimensions(self) -> (f32, f32) {
        match self {
            CardSize::Small => (160.0, 90.0),
            CardSize::Medium => (220.0, 124.0),
            CardSize::Large => (300.0, 169.0),
            CardSize::XLarge => (400.0, 225.0),
        }
    }

    pub fn next(self) -> Self {
        match self {
            CardSize::Small => CardSize::Medium,
            CardSize::Medium => CardSize::Large,
            CardSize::Large => CardSize::XLarge,
            CardSize::XLarge => CardSize::Small,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            CardSize::Small => CardSize::XLarge,
            CardSize::Medium => CardSize::Small,
            CardSize::Large => CardSize::Medium,
            CardSize::XLarge => CardSize::Large,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            CardSize::Small => "Small",
            CardSize::Medium => "Medium",
            CardSize::Large => "Large",
            CardSize::XLarge => "Extra Large",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DateFilter {
    AllTime,
    Last24Hours,
    Last7Days,
    Last30Days,
}

impl DateFilter {
    pub fn label(self) -> &'static str {
        match self {
            DateFilter::AllTime => "All Time",
            DateFilter::Last24Hours => "Last 24 Hours",
            DateFilter::Last7Days => "Last 7 Days",
            DateFilter::Last30Days => "Last 30 Days",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DurationFilter {
    All,
    Short,
    Medium,
    Long,
}

impl DurationFilter {
    pub fn label(self) -> &'static str {
        match self {
            DurationFilter::All => "All Durations",
            DurationFilter::Short => "Short (<30s)",
            DurationFilter::Medium => "Medium (30s-5m)",
            DurationFilter::Long => "Long (>5m)",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SizeFilter {
    All,
    Small,
    Medium,
    Large,
}

impl SizeFilter {
    pub fn label(self) -> &'static str {
        match self {
            SizeFilter::All => "All Sizes",
            SizeFilter::Small => "Small (<10MB)",
            SizeFilter::Medium => "Medium (10-50MB)",
            SizeFilter::Large => "Large (>50MB)",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortBy {
    DateNewest,
    DateOldest,
    NameAZ,
    NameZA,
    SizeLarge,
    SizeSmall,
    DurationLong,
    DurationShort,
}

impl SortBy {
    pub fn label(self) -> &'static str {
        match self {
            SortBy::DateNewest => "Date (Newest First)",
            SortBy::DateOldest => "Date (Oldest First)",
            SortBy::NameAZ => "Name (A-Z)",
            SortBy::NameZA => "Name (Z-A)",
            SortBy::SizeLarge => "Size (Largest First)",
            SortBy::SizeSmall => "Size (Smallest First)",
            SortBy::DurationLong => "Duration (Longest First)",
            SortBy::DurationShort => "Duration (Shortest First)",
        }
    }
}

pub fn show_gallery_gui(event_tx: TokioSender<AppEvent>, config: Config) {
    crate::gui::manager::send_gui_message(crate::gui::manager::GuiMessage::ShowGallery(
        event_tx, config,
    ));
}

#[derive(Clone)]
struct VideoEntry {
    path: PathBuf,
    save_root: PathBuf,
    is_external: bool,
    game: String,
    filename: String,
    size_mb: f64,
    modified: SystemTime,
    metadata: VideoFileMetadata,
    is_clipped: bool, // True if video came from a "Clipped-" folder
}

struct ThumbnailResult {
    video_path: PathBuf,
    image: Option<RgbaImage>,
    error: Option<String>,
}

struct ThumbnailStripResult {
    video_path: PathBuf,
    strip: Option<ThumbnailStrip>,
    error: Option<String>,
}

enum DialogResult {
    ImportVideo(Option<PathBuf>),
    SaveOutputPath(Option<PathBuf>),
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

pub(super) const THUMBNAIL_STRIP_COUNT: usize = 20;
pub(super) const THUMBNAIL_STRIP_WIDTH: u32 = 160;

pub(super) struct ThumbnailStrip {
    pub(super) thumbnails: Vec<(f64, RgbaImage)>,
}

impl ThumbnailStrip {
    pub(super) fn new(thumbnails: Vec<(f64, RgbaImage)>, _duration_secs: f64) -> Self {
        Self { thumbnails }
    }

    #[allow(dead_code)]
    pub(super) fn nearest(&self, time_secs: f64) -> Option<&RgbaImage> {
        if self.thumbnails.is_empty() {
            return None;
        }

        let idx = self
            .thumbnails
            .partition_point(|(t, _)| *t <= time_secs)
            .saturating_sub(1);

        self.thumbnails.get(idx).map(|(_, img)| img)
    }
}

pub(super) struct EditorState {
    video: VideoEntry,
    current_time_secs: f64,
    is_playing: bool,
    last_tick: Instant,
    cut_points: Vec<f64>,
    snippet_enabled: Vec<bool>,
    selected_cut_point: Option<usize>,
    target_size_mb: u32,
    /// Whether target_size_mb was manually changed by the user.
    /// If false, export will use stream copy (no re-encoding).
    target_size_manually_adjusted: bool,
    audio_bitrate_kbps: u32,
    use_hardware_acceleration: bool,
    preferred_encoder: EncoderType,
    /// Output resolution. None means auto (based on target size).
    output_width: Option<u32>,
    output_height: Option<u32>,
    /// If true, resolution is automatically calculated based on target size.
    use_auto_resolution: bool,
    /// Output frame rate. None means use original.
    output_fps: Option<f64>,
    preview_texture: Option<egui::TextureHandle>,
    preview_frame_in_flight: bool,
    pending_preview_request: Option<f64>,
    last_requested_preview_time: Option<f64>,
    status_message: Option<String>,
    error_message: Option<String>,
    export_state: Option<ExportState>,
    export_output: Option<PathBuf>,
    custom_output_path: Option<PathBuf>,
    playback: PlaybackController,
    was_playing_before_scrub: bool,
    last_scrub_time: Option<Instant>,
    last_scrub_position: Option<f64>,
    selected_snippet_index: Option<usize>,
    thumbnail_strip: Option<ThumbnailStrip>,
    thumbnail_strip_loading: bool,
}

impl EditorState {
    fn new(video: VideoEntry, preferred_encoder: EncoderType, use_software_encoder: bool) -> Self {
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
            last_tick: Instant::now(),
            cut_points: Vec::new(),
            snippet_enabled: vec![true],
            selected_cut_point: None,
            target_size_mb,
            target_size_manually_adjusted: false,
            audio_bitrate_kbps: DEFAULT_AUDIO_BITRATE_KBPS,
            use_hardware_acceleration: !use_software_encoder,
            preferred_encoder,
            output_width: None,
            output_height: None,
            use_auto_resolution: true,
            output_fps: None,
            preview_texture: None,
            preview_frame_in_flight: false,
            pending_preview_request: None,
            last_requested_preview_time: None,
            status_message: None,
            error_message: None,
            export_state: None,
            export_output: None,
            custom_output_path: None,
            playback,
            was_playing_before_scrub: false,
            last_scrub_time: None,
            last_scrub_position: None,
            selected_snippet_index: Some(0),
            thumbnail_strip: None,
            thumbnail_strip_loading: false,
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

    /// Maximum output size in MB based on proportion of enabled segments.
    /// If half the video is disabled, max output is half the original size.
    fn max_output_size_mb(&self) -> u32 {
        let total_duration = self.duration_secs();
        if total_duration <= 0.0 {
            return 1;
        }
        let kept_duration = self.kept_duration_secs();
        let proportion = kept_duration / total_duration;
        let max_size = (self.video.size_mb * proportion).ceil().max(1.0);
        max_size as u32
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

    /// Compute effective output resolution.
    /// If use_auto_resolution is true, calculates based on target size vs original.
    /// Otherwise returns manually set resolution or original if not set.
    fn effective_output_resolution(&self) -> (u32, u32) {
        let orig_w = self.video.metadata.width;
        let orig_h = self.video.metadata.height;

        if self.use_auto_resolution {
            // Calculate resolution scale based on target size vs original size
            let kept_duration = self.kept_duration_secs();
            let total_duration = self.duration_secs();
            let kept_ratio = if total_duration > 0.0 {
                (kept_duration / total_duration).clamp(0.1, 1.0)
            } else {
                1.0
            };

            // Effective "original" size for kept portion
            let effective_orig_mb = self.video.size_mb * kept_ratio;
            let target_mb = self.target_size_mb as f64;

            // Scale factor: if target is smaller, reduce resolution
            // Use square root since video compression is roughly proportional to pixel count
            let scale = if effective_orig_mb > 0.0 && target_mb < effective_orig_mb {
                (target_mb / effective_orig_mb).sqrt().clamp(0.5, 1.0)
            } else {
                1.0
            };

            // Apply scale and round to nearest multiple of 2 (required for many codecs)
            let new_w = ((orig_w as f64 * scale) as u32 / 2 * 2).max(640);
            let new_h = ((orig_h as f64 * scale) as u32 / 2 * 2).max(360);
            (new_w, new_h)
        } else if let (Some(w), Some(h)) = (self.output_width, self.output_height) {
            (w, h)
        } else {
            (orig_w, orig_h)
        }
    }

    /// Get effective output FPS. Returns manual setting or original.
    fn effective_output_fps(&self) -> f64 {
        let fps = self.output_fps.unwrap_or(self.video.metadata.fps);
        if fps.is_finite() {
            fps.clamp(1.0, 240.0)
        } else {
            60.0
        }
    }
}

pub struct ClipCompressApp {
    save_directory: PathBuf,
    cache_directory: PathBuf,
    videos_by_game: Vec<(String, Vec<VideoEntry>)>,
    filter_game: String,
    // Enhanced filtering
    search_query: String,
    date_filter: DateFilter,
    duration_filter: DurationFilter,
    size_filter: SizeFilter,
    show_clipped_only: bool,
    sort_by: SortBy,
    // Card sizing
    card_size: CardSize,
    // Collapsible sections
    collapsed_games: HashSet<String>,
    loaded: bool,
    scan_error: Option<String>,
    thumbnails: FxHashMap<PathBuf, egui::TextureHandle>,
    thumbnails_generating: HashSet<PathBuf>,
    thumbnail_tx: Sender<ThumbnailResult>,
    thumbnail_rx: Receiver<ThumbnailResult>,
    #[allow(dead_code)]
    thumbnail_strip_tx: Sender<ThumbnailStripResult>,
    thumbnail_strip_rx: Receiver<ThumbnailStripResult>,
    dialog_tx: Sender<DialogResult>,
    dialog_rx: Receiver<DialogResult>,
    import_dialog_pending: bool,
    save_dialog_pending: bool,
    editor: Option<EditorState>,
    pub selection_mode: bool,
    pub selected_videos: HashSet<PathBuf>,
    pub delete_slider_progress: f32,
    delete_hold_started_at: Option<Instant>,
    preferred_export_encoder: EncoderType,
    use_software_encoder: bool,
    last_thumbnail_check: Instant,
}

pub type GalleryApp = ClipCompressApp;

impl ClipCompressApp {
    pub fn new(config: &Config, _event_tx: TokioSender<AppEvent>) -> Self {
        let save_directory = PathBuf::from(&config.general.save_directory);
        let cache_directory = save_directory.join(".cache");
        let (thumbnail_tx, thumbnail_rx) = mpsc::channel();
        let (thumbnail_strip_tx, thumbnail_strip_rx) = mpsc::channel();
        let (dialog_tx, dialog_rx) = mpsc::channel();

        Self {
            save_directory,
            cache_directory,
            videos_by_game: Vec::new(),
            filter_game: ALL_GAMES_FILTER.to_string(),
            // Enhanced filtering - initialized with defaults
            search_query: String::new(),
            date_filter: DateFilter::AllTime,
            duration_filter: DurationFilter::All,
            size_filter: SizeFilter::All,
            show_clipped_only: false,
            sort_by: SortBy::DateNewest,
            // Card sizing
            card_size: CardSize::Medium,
            // Collapsible sections
            collapsed_games: HashSet::new(),
            loaded: false,
            scan_error: None,
            thumbnails: FxHashMap::default(),
            thumbnails_generating: HashSet::new(),
            thumbnail_tx,
            thumbnail_rx,
            thumbnail_strip_tx,
            thumbnail_strip_rx,
            dialog_tx,
            dialog_rx,
            import_dialog_pending: false,
            save_dialog_pending: false,
            editor: None,
            selection_mode: false,
            selected_videos: HashSet::new(),
            delete_slider_progress: 0.0,
            delete_hold_started_at: None,
            preferred_export_encoder: config.video.encoder,
            use_software_encoder: config.general.use_software_encoder,
            last_thumbnail_check: Instant::now(),
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

        let mut paths = Vec::with_capacity(256);
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

        let mut grouped: Vec<(String, Vec<VideoEntry>)> = Vec::with_capacity(16);
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
        let raw_game = relative
            .parent()
            .map(|parent| parent.to_string_lossy().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "Desktop".to_string());

        // Normalize game name by stripping "Clipped-" prefix to consolidate sections
        let game = Self::normalize_game_name(&raw_game);
        let is_clipped = raw_game.starts_with("Clipped-");

        Ok(VideoEntry {
            path,
            save_root: base_dir.to_path_buf(),
            is_external: false,
            game,
            filename,
            size_mb: metadata.len() as f64 / (1024.0 * 1024.0),
            modified: metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            metadata: video_metadata,
            is_clipped,
        })
    }

    fn normalize_game_name(game: &str) -> String {
        game.strip_prefix("Clipped-")
            .map(|s| s.to_string())
            .unwrap_or_else(|| game.to_string())
    }

    fn build_external_video_entry(path: &Path, save_root: &Path) -> anyhow::Result<VideoEntry> {
        let metadata = std::fs::metadata(path)
            .map_err(anyhow::Error::from)
            .with_context(|| format!("Failed to read metadata for external video {:?}", path))?;
        let video_metadata = probe_video_file(path)
            .with_context(|| format!("Failed to probe external video file {:?}", path))?;
        let filename = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "video.mp4".to_string());

        Ok(VideoEntry {
            path: path.to_path_buf(),
            save_root: save_root.to_path_buf(),
            is_external: true,
            game: "External Files".to_string(),
            filename,
            size_mb: metadata.len() as f64 / (1024.0 * 1024.0),
            modified: metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            metadata: video_metadata,
            is_clipped: false,
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

    /// Check for newly generated thumbnails for videos that don't have them yet
    /// Called periodically to detect thumbnails created after initial scan
    fn check_for_new_thumbnails(&mut self, ctx: &egui::Context) {
        let now = Instant::now();

        // Only check every 5 seconds to avoid excessive polling
        // Reduced from 3 seconds since thumbnails are generated asynchronously
        if now.duration_since(self.last_thumbnail_check) < Duration::from_secs(5) {
            return;
        }

        self.last_thumbnail_check = now;

        // Find videos without thumbnails (limit to 3 per check to avoid blocking)
        let videos_to_check: Vec<_> = self
            .videos_by_game
            .iter()
            .flat_map(|(_, videos)| videos.iter())
            .filter(|video| !self.thumbnails.contains_key(&video.path))
            .take(3)
            .collect();

        if videos_to_check.is_empty() {
            return;
        }

        // Check if thumbnails now exist and load them
        let mut newly_found = Vec::new();
        for video in videos_to_check {
            let thumb_path = self.get_thumb_path(&video.path);
            if thumb_path.exists() {
                if let Ok(image) = load_rgba_image_from_path(&thumb_path) {
                    newly_found.push((video.path.clone(), video.filename.clone(), image));
                }
            }
        }

        // Load any newly found thumbnails
        for (video_path, filename, image) in newly_found {
            self.insert_thumbnail_texture(ctx, video_path, &filename, image);
        }
    }

    fn schedule_thumbnail_generation(&mut self, video_path: &PathBuf) {
        if self.thumbnails.contains_key(video_path)
            || self.thumbnails_generating.contains(video_path)
        {
            return;
        }

        self.thumbnails_generating.insert(video_path.clone());
        let tx = self.thumbnail_tx.clone();
        let video_path = video_path.clone();
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

    fn unload_browser_thumbnails(&mut self) {
        self.thumbnails.clear();
    }

    fn close_editor(&mut self) {
        if let Some(mut editor) = self.editor.take() {
            editor.playback.release_idle_resources();
            editor.preview_texture = None;
            editor.thumbnail_strip = None;
            editor.thumbnail_strip_loading = false;
            editor.pending_preview_request = None;
            editor.last_requested_preview_time = None;
        }
    }

    pub fn release_all_gui_resources(&mut self) {
        self.close_editor();
        self.unload_browser_thumbnails();
        self.thumbnails_generating.clear();
    }

    fn request_import_video_dialog(&mut self) {
        if self.import_dialog_pending {
            return;
        }
        self.import_dialog_pending = true;
        let tx = self.dialog_tx.clone();
        std::thread::spawn(move || {
            let picked = rfd::FileDialog::new()
                .add_filter("Video", &["mp4", "mov", "mkv", "webm", "avi", "m4v"])
                .pick_file();
            let _ = tx.send(DialogResult::ImportVideo(picked));
        });
    }

    fn request_save_output_dialog(&mut self, output_path: PathBuf) {
        if self.save_dialog_pending {
            return;
        }
        self.save_dialog_pending = true;
        let tx = self.dialog_tx.clone();
        std::thread::spawn(move || {
            let suggested_name = output_path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "clip.mp4".to_string());
            let picked = rfd::FileDialog::new()
                .add_filter("MP4", &["mp4"])
                .set_file_name(&suggested_name)
                .set_directory(
                    output_path
                        .parent()
                        .unwrap_or_else(|| std::path::Path::new(".")),
                )
                .save_file();
            let _ = tx.send(DialogResult::SaveOutputPath(picked));
        });
    }

    fn apply_dialog_results(&mut self, requested_preview: &mut Option<f64>) {
        while let Ok(dialog_result) = self.dialog_rx.try_recv() {
            match dialog_result {
                DialogResult::ImportVideo(path) => {
                    self.import_dialog_pending = false;
                    if let Some(import_path) = path {
                        match Self::build_external_video_entry(&import_path, &self.save_directory) {
                            Ok(video) => {
                                self.open_editor(video);
                                *requested_preview = Some(0.0);
                            }
                            Err(err) => {
                                self.scan_error = Some(format!(
                                    "Failed to open video {:?}: {err:#}",
                                    import_path
                                ));
                            }
                        }
                    }
                }
                DialogResult::SaveOutputPath(path) => {
                    self.save_dialog_pending = false;
                    if let (Some(editor), Some(output_path)) = (self.editor.as_mut(), path) {
                        editor.custom_output_path = Some(output_path);
                    }
                }
            }
        }
    }

    fn queue_preview_request(&mut self, timestamp_secs: f64) {
        let Some(editor) = self.editor.as_mut() else {
            return;
        };

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
        if editor.playback.is_frame_request_in_flight() || editor.preview_frame_in_flight {
            editor.pending_preview_request = Some(timestamp_secs);
            return;
        }

        // More aggressive debouncing during scrubbing (150ms instead of 100ms)
        // This prevents overwhelming the decoder with requests during fast scrub
        if let Some(last_requested) = editor.last_requested_preview_time {
            if editor.preview_texture.is_some() && (last_requested - timestamp_secs).abs() < 0.08 {
                return;
            }
        }

        editor.last_requested_preview_time = Some(timestamp_secs);
        editor.preview_frame_in_flight = true;
        editor.pending_preview_request = None;
        editor.playback.request_preview_frame_fast(timestamp_secs);
    }

    fn poll_background_work(&mut self, ctx: &egui::Context) -> Option<f64> {
        let mut follow_up_preview = None;
        let has_editor_pending_activity = self
            .editor
            .as_ref()
            .map(|editor| editor.playback.has_pending_activity())
            .unwrap_or(false);

        // Check for newly generated thumbnails periodically
        if !has_editor_pending_activity {
            self.check_for_new_thumbnails(ctx);
        }

        while let Ok(result) = self.thumbnail_strip_rx.try_recv() {
            if let Some(editor) = self.editor.as_mut() {
                if editor.video.path == result.video_path {
                    editor.thumbnail_strip_loading = false;
                    if let Some(strip) = result.strip {
                        tracing::info!(
                            "Generated thumbnail strip with {} frames",
                            strip.thumbnails.len()
                        );
                        editor.thumbnail_strip = Some(strip);
                    } else if let Some(error) = result.error {
                        tracing::warn!("Failed to generate thumbnail strip: {error}");
                    }
                }
            }
        }

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
            editor.playback.poll();

            // Live playback path: drain the frame queue for frames due by now.
            if editor.is_playing {
                let wall_time = editor.playback.playback_position_secs();
                let queue_len = editor.playback.cached_frame_count();
                if let Some(image) = editor.playback.take_playback_frame() {
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

            // Static / single-frame preview path (paused).
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
        self.apply_dialog_results(&mut requested_preview);

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

        // Handle keyboard navigation in browser mode
        if self.editor.is_none() {
            self.handle_keyboard_navigation(ctx);
        }

        for video_path in browser_outcome.thumbnails_to_generate {
            self.schedule_thumbnail_generation(&video_path);
        }

        if browser_outcome.refresh_requested || editor_outcome.refresh_browser {
            self.refresh();
        }

        if browser_outcome.request_import_video_dialog {
            self.request_import_video_dialog();
        }

        if let Some(output_path) = editor_outcome.request_save_output_dialog {
            self.request_save_output_dialog(output_path);
        }

        if let Some(video) = browser_outcome.selected_video {
            self.open_editor(video);
            requested_preview = Some(0.0);
        }

        if !browser_outcome.videos_to_delete.is_empty() {
            for video in browser_outcome.videos_to_delete {
                if let Err(e) = std::fs::remove_file(&video.path) {
                    warn!("Failed to delete video {:?}: {}", video.path, e);
                } else {
                    info!("Deleted video {:?}", video.path);
                    let thumb_path = self.get_thumb_path(&video.path);
                    if thumb_path.exists() {
                        let _ = std::fs::remove_file(&thumb_path);
                    }
                }
            }
            self.selected_videos.clear();
            self.selection_mode = false;
            self.refresh();
        }

        if let Some(video) = browser_outcome.video_to_open {
            if let Err(e) = open_path(&video.path) {
                warn!("Failed to open video {:?}: {}", video.path, e);
            }
        }

        if editor_outcome.back_to_browser {
            self.close_editor();
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
                .filter(|e| e.is_playing)
                .map(|e| (1000.0 / e.playback.playback_fps()).clamp(8.0, 50.0) as u64)
                .unwrap_or(80);
            ctx.request_repaint_after(Duration::from_millis(repaint_ms));
        }
    }

    fn should_repaint(&self) -> bool {
        let (
            editor_is_playing,
            preview_frame_in_flight,
            thumbnail_strip_loading,
            playback_pending_activity,
            export_active,
        ) = self
            .editor
            .as_ref()
            .map(|editor| {
                (
                    editor.is_playing,
                    editor.preview_frame_in_flight,
                    editor.thumbnail_strip_loading,
                    editor.playback.has_pending_activity(),
                    editor.has_active_export(),
                )
            })
            .unwrap_or((false, false, false, false, false));

        should_repaint_gallery(
            self.delete_hold_started_at.is_some(),
            !self.thumbnails_generating.is_empty(),
            self.import_dialog_pending,
            self.save_dialog_pending,
            editor_is_playing,
            preview_frame_in_flight,
            thumbnail_strip_loading,
            playback_pending_activity,
            export_active,
        )
    }

    fn open_editor(&mut self, video: VideoEntry) {
        info!("Opening Clip & Compress editor for {:?}", video.path);
        self.unload_browser_thumbnails();
        self.editor = Some(EditorState::new(
            video,
            self.preferred_export_encoder,
            self.use_software_encoder,
        ));
    }

    fn render_browser(&mut self, ui: &mut egui::Ui) -> BrowserUiOutcome {
        browser::render_browser_ui(self, ui)
    }

    fn render_editor(&mut self, ui: &mut egui::Ui) -> EditorUiOutcome {
        editor::render_editor_ui(self, ui)
    }

    pub fn refresh(&mut self) {
        self.close_editor();
        self.loaded = false;
        self.scan_error = None;
        self.videos_by_game.clear();
        self.thumbnails.clear();
        self.thumbnails_generating.clear();
        self.delete_hold_started_at = None;
        self.delete_slider_progress = 0.0;
    }

    fn handle_keyboard_navigation(&mut self, ctx: &egui::Context) {
        use egui::Key;

        // Card size shortcuts: Ctrl + Plus/Minus/Zero
        ctx.input(|input| {
            if input.modifiers.ctrl || input.modifiers.command {
                if input.key_pressed(Key::Plus) || input.key_pressed(Key::Equals) {
                    self.card_size = self.card_size.next();
                } else if input.key_pressed(Key::Minus) {
                    self.card_size = self.card_size.prev();
                } else if input.key_pressed(Key::Num0) {
                    self.card_size = CardSize::Medium;
                }
            }

            // Selection shortcuts
            if self.selection_mode {
                // Escape to exit selection mode
                if input.key_pressed(Key::Escape) {
                    self.selection_mode = false;
                    self.selected_videos.clear();
                    self.delete_slider_progress = 0.0;
                }
            }
        });
    }
}

impl Drop for ClipCompressApp {
    fn drop(&mut self) {
        self.release_all_gui_resources();
    }
}

#[derive(Default)]
struct BrowserUiOutcome {
    thumbnails_to_generate: Vec<PathBuf>,
    selected_video: Option<VideoEntry>,
    videos_to_delete: Vec<VideoEntry>,
    video_to_open: Option<VideoEntry>,
    request_import_video_dialog: bool,
    refresh_requested: bool,
}

#[derive(Default)]
struct EditorUiOutcome {
    preview_request: Option<f64>,
    fast_preview_request: Option<f64>,
    request_save_output_dialog: Option<PathBuf>,
    back_to_browser: bool,
    refresh_browser: bool,
}

fn collect_video_paths(dir: &Path, cache_dir: &Path, output: &mut Vec<PathBuf>) {
    utils::collect_video_paths_impl(dir, cache_dir, output);
}

fn toggle_editor_playback(editor: &mut EditorState) {
    if editor.is_playing {
        editor.playback.pause_at(editor.current_time_secs);
        editor.is_playing = false;
        return;
    }

    editor.last_tick = Instant::now();
    editor.is_playing = true;
    editor.preview_frame_in_flight = false;
    editor.pending_preview_request = None;
    editor.playback.play_from(editor.current_time_secs);
}

fn seek_editor(editor: &mut EditorState, outcome: &mut EditorUiOutcome, delta_secs: f64) {
    editor.playback.pause_at(editor.current_time_secs);
    editor.is_playing = false;
    editor.current_time_secs =
        (editor.current_time_secs + delta_secs).clamp(0.0, editor.duration_secs());
    outcome.preview_request = Some(editor.current_time_secs);
}

fn clamp_selected_snippet_index(editor: &mut EditorState) {
    editor_panels::clamp_selected_snippet_index_impl(editor);
}

fn render_editor_workspace(
    ui: &mut egui::Ui,
    editor: &mut EditorState,
    outcome: &mut EditorUiOutcome,
) {
    editor_panels::render_editor_workspace_impl(ui, editor, outcome);
}

fn update_playback_clock(editor: &mut EditorState, _outcome: &mut EditorUiOutcome) {
    if !editor.is_playing || editor.has_active_export() {
        editor.last_tick = Instant::now();
        return;
    }

    editor.current_time_secs = editor.playback.playback_position_secs();
    if editor.current_time_secs >= editor.duration_secs() {
        editor.current_time_secs = editor.duration_secs();
        editor.is_playing = false;
        editor.playback.pause_at(editor.current_time_secs);
    }
}

fn start_export(editor: &mut EditorState) {
    let kept_ranges = editor.kept_ranges();
    if kept_ranges.is_empty() {
        editor.error_message =
            Some("At least one snippet must stay enabled for export".to_string());
        return;
    }

    let output_path = editor
        .custom_output_path
        .clone()
        .unwrap_or_else(|| build_clipped_output_path(&editor.video));
    let (progress_tx, progress_rx) = mpsc::channel();
    let cancel_flag = Arc::new(AtomicBool::new(false));

    spawn_clip_export(
        ClipExportRequest {
            input_path: editor.video.path.clone(),
            output_path,
            keep_ranges: kept_ranges,
            target_size_mb: editor.target_size_mb,
            audio_bitrate_kbps: editor.audio_bitrate_kbps,
            use_hardware_acceleration: editor.use_hardware_acceleration,
            preferred_encoder: editor.preferred_encoder,
            metadata: editor.video.metadata.clone(),
            stream_copy: !editor.target_size_manually_adjusted,
            output_width: if editor.use_auto_resolution {
                let (w, _h) = editor.effective_output_resolution();
                Some(w)
            } else {
                editor.output_width
            },
            output_height: if editor.use_auto_resolution {
                let (_w, h) = editor.effective_output_resolution();
                Some(h)
            } else {
                editor.output_height
            },
            output_fps: Some(editor.effective_output_fps()),
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
}

fn poll_editor_export_updates(
    editor: &mut EditorState,
    outcome: &mut EditorUiOutcome,
    ctx: &egui::Context,
) -> bool {
    let mut finished_path = None;
    let mut failed_message = None;
    let mut cancelled = false;
    let mut received_update = false;

    if let Some(export) = editor.export_state.as_mut() {
        while let Ok(update) = export.progress_rx.try_recv() {
            received_update = true;
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
        error!("Clip export failed (UI): {message}");
        editor.export_state = None;
        editor.error_message = Some(message);
        show_toast(ToastKind::Error, "Clip export failed");
    } else if cancelled {
        editor.export_state = None;
        editor.status_message = Some("Export cancelled".to_string());
        show_toast(ToastKind::Warning, "Clip export cancelled");
    }

    // Request immediate repaint if we received any progress update
    if received_update && editor.has_active_export() {
        ctx.request_repaint();
    }

    received_update
}

fn render_completion_screen(ui: &mut egui::Ui, editor: &mut EditorState) -> EditorUiOutcome {
    let mut outcome = EditorUiOutcome::default();
    let Some(output_path) = editor.export_output.clone() else {
        return outcome;
    };

    ui.horizontal(|ui| {
        if ui.button("< Back to Videos (Esc)").clicked() {
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
    utils::open_path_impl(path)
}

fn build_clipped_output_path(video: &VideoEntry) -> PathBuf {
    utils::build_clipped_output_path_impl(video)
}

fn load_rgba_image_from_path(path: &Path) -> anyhow::Result<RgbaImage> {
    utils::load_rgba_image_from_path_impl(path)
}

fn color_image_from_rgba(image: &RgbaImage) -> egui::ColorImage {
    utils::color_image_from_rgba_impl(image)
}

fn format_size_mb(size_mb: f64) -> String {
    utils::format_size_mb_impl(size_mb)
}

fn format_compact_duration(seconds: f64) -> String {
    utils::format_compact_duration_impl(seconds)
}

fn format_timestamp_precise(seconds: f64) -> String {
    utils::format_timestamp_precise_impl(seconds)
}

fn clear_cut_points(editor: &mut EditorState) {
    utils::clear_cut_points_impl(editor);
}

fn add_cut_point(editor: &mut EditorState, time_secs: f64) -> bool {
    utils::add_cut_point_impl(editor, time_secs)
}

fn remove_cut_point(editor: &mut EditorState, index: usize) {
    utils::remove_cut_point_impl(editor, index);
}

fn snippet_segments(
    duration_secs: f64,
    cut_points: &[f64],
    snippet_enabled: &[bool],
) -> Vec<SnippetSegment> {
    utils::snippet_segments_impl(duration_secs, cut_points, snippet_enabled)
}

fn enabled_time_ranges(
    duration_secs: f64,
    cut_points: &[f64],
    snippet_enabled: &[bool],
) -> Vec<TimeRange> {
    utils::enabled_time_ranges_impl(duration_secs, cut_points, snippet_enabled)
}

fn clamp_to_enabled_playback_time(
    current_time_secs: f64,
    duration_secs: f64,
    cut_points: &[f64],
    snippet_enabled: &[bool],
) -> f64 {
    utils::clamp_to_enabled_playback_time_impl(
        current_time_secs,
        duration_secs,
        cut_points,
        snippet_enabled,
    )
}

fn estimate_export_bitrates_from_editor(
    target_size_mb: u32,
    kept_duration_secs: f64,
    has_audio: bool,
    requested_audio_bitrate_kbps: u32,
    num_segments: usize,
    use_hardware_acceleration: bool,
) -> (u32, u32) {
    utils::estimate_export_bitrates_from_editor_impl(
        target_size_mb,
        kept_duration_secs,
        has_audio,
        requested_audio_bitrate_kbps,
        num_segments,
        use_hardware_acceleration,
    )
}

fn quality_estimate(metadata: &VideoFileMetadata, video_kbps: u32) -> (&'static str, usize) {
    utils::quality_estimate_impl(metadata, video_kbps)
}

fn time_to_x(rect: egui::Rect, time_secs: f64, duration_secs: f64) -> f32 {
    utils::time_to_x_impl(rect, time_secs, duration_secs)
}

fn x_to_time(rect: egui::Rect, x: f32, duration_secs: f64) -> f64 {
    utils::x_to_time_impl(rect, x, duration_secs)
}

#[allow(clippy::too_many_arguments)]
fn should_repaint_gallery(
    delete_hold_active: bool,
    browser_thumbnail_work_active: bool,
    import_dialog_pending: bool,
    save_dialog_pending: bool,
    editor_is_playing: bool,
    preview_frame_in_flight: bool,
    thumbnail_strip_loading: bool,
    playback_pending_activity: bool,
    export_active: bool,
) -> bool {
    delete_hold_active
        || browser_thumbnail_work_active
        || import_dialog_pending
        || save_dialog_pending
        || editor_is_playing
        || preview_frame_in_flight
        || thumbnail_strip_loading
        || playback_pending_activity
        || export_active
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

    #[test]
    fn gallery_repaint_stays_idle_when_editor_is_paused() {
        assert!(
            !should_repaint_gallery(false, false, false, false, false, false, false, false, false),
            "Paused gallery/editor should not keep repainting without active work"
        );
    }

    #[test]
    fn gallery_repaint_runs_for_live_editor_work() {
        assert!(should_repaint_gallery(
            false, false, false, false, true, false, false, false, false
        ));
        assert!(should_repaint_gallery(
            false, false, false, false, false, true, false, false, false
        ));
        assert!(should_repaint_gallery(
            false, false, false, false, false, false, true, false, false
        ));
        assert!(should_repaint_gallery(
            false, false, false, false, false, false, false, true, false
        ));
        assert!(should_repaint_gallery(
            false, false, false, false, false, false, false, false, true
        ));
    }

    #[test]
    fn gallery_repaint_runs_for_background_results_that_need_polling() {
        assert!(should_repaint_gallery(
            false, true, false, false, false, false, false, false, false
        ));
        assert!(should_repaint_gallery(
            false, false, true, false, false, false, false, false, false
        ));
        assert!(should_repaint_gallery(
            false, false, false, true, false, false, false, false, false
        ));
    }
}
