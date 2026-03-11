use eframe::egui;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::SystemTime;
use tokio::sync::mpsc::Sender;
use tracing::{debug, error, info};

use crate::config::Config;
use crate::platform::AppEvent;

/// Shows the clip gallery GUI window.
///
/// Spawns a new egui window for browsing and managing saved clips.
/// Clips are organized by game folder if game detection is enabled.
///
/// # Arguments
///
/// * `event_tx` - Channel to send events to the main application.
pub fn show_gallery_gui(event_tx: Sender<AppEvent>) {
    crate::gui::manager::send_gui_message(crate::gui::manager::GuiMessage::ShowGallery(event_tx));
}

#[derive(Clone)]
struct VideoEntry {
    path: PathBuf,
    filename: String,
    size_mb: f64,
    modified: SystemTime,
}

pub struct GalleryApp {
    save_directory: PathBuf,
    cache_directory: PathBuf,
    videos_by_game: Vec<(String, Vec<VideoEntry>)>,
    expanded_games: HashMap<String, bool>,
    selected: Option<(String, usize)>,
    loaded: bool,
    thumbnails: HashMap<PathBuf, egui::TextureHandle>,
    thumbnails_generating: HashSet<PathBuf>,
}

impl GalleryApp {
    pub fn new(config: &Config, _event_tx: Sender<AppEvent>) -> Self {
        let save_directory = PathBuf::from(&config.general.save_directory);
        let cache_directory = save_directory.join(".cache");
        Self {
            save_directory,
            cache_directory,
            videos_by_game: Vec::new(),
            expanded_games: HashMap::new(),
            selected: None,
            loaded: false,
            thumbnails: HashMap::new(),
            thumbnails_generating: HashSet::new(),
        }
    }

    fn get_thumb_path(&self, video_path: &PathBuf) -> PathBuf {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        video_path.hash(&mut hasher);
        let hash = hasher.finish();
        self.cache_directory.join(format!("{:016x}.jpg", hash))
    }

    fn scan_videos(&mut self, ctx: &egui::Context) {
        info!("Scanning videos in: {:?}", self.save_directory);
        self.videos_by_game.clear();
        self.thumbnails.clear();

        if !self.save_directory.exists() {
            debug!("Save directory does not exist yet");
            self.loaded = true;
            return;
        }

        let mut videos_by_game: HashMap<String, Vec<VideoEntry>> = HashMap::new();

        fn scan_dir(
            dir: &PathBuf,
            videos: &mut HashMap<String, Vec<VideoEntry>>,
            base_dir: &PathBuf,
            cache_dir: &PathBuf,
        ) {
            if let Ok(read_dir) = std::fs::read_dir(dir) {
                for entry in read_dir.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        if path == *cache_dir {
                            continue;
                        }
                        scan_dir(&path, videos, base_dir, cache_dir);
                    } else if path.extension().map(|e| e == "mp4").unwrap_or(false) {
                        if let Ok(metadata) = entry.metadata() {
                            let filename = path
                                .file_name()
                                .map(|n| n.to_string_lossy().to_string())
                                .unwrap_or_default();

                            let relative = path.strip_prefix(base_dir).unwrap_or(&path);
                            let game = relative
                                .parent()
                                .map(|p| p.to_string_lossy().to_string())
                                .filter(|s| !s.is_empty())
                                .unwrap_or_else(|| "Desktop".to_string());

                            let size_mb = metadata.len() as f64 / (1024.0 * 1024.0);
                            let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);

                            videos.entry(game).or_default().push(VideoEntry {
                                path,
                                filename,
                                size_mb,
                                modified,
                            });
                        }
                    }
                }
            }
        }

        scan_dir(
            &self.save_directory,
            &mut videos_by_game,
            &self.save_directory,
            &self.cache_directory,
        );

        let mut sorted: Vec<_> = videos_by_game.into_iter().collect();
        sorted.sort_by(|a, b| {
            if a.0 == "Desktop" {
                std::cmp::Ordering::Less
            } else if b.0 == "Desktop" {
                std::cmp::Ordering::Greater
            } else {
                a.0.cmp(&b.0)
            }
        });

        for (_, videos) in &mut sorted {
            videos.sort_by(|a, b| b.modified.cmp(&a.modified));
        }

        self.videos_by_game = sorted;
        self.loaded = true;

        let total: usize = self.videos_by_game.iter().map(|(_, v)| v.len()).sum();
        info!(
            "Found {} videos in {} games",
            total,
            self.videos_by_game.len()
        );

        self.load_thumbnails(ctx);
    }

    fn load_thumbnails(&mut self, ctx: &egui::Context) {
        for (_, videos) in &self.videos_by_game {
            for video in videos {
                if self.thumbnails.contains_key(&video.path) {
                    continue;
                }

                let thumb_path = self.get_thumb_path(&video.path);
                if thumb_path.exists() {
                    if let Ok(img_data) = std::fs::read(&thumb_path) {
                        if let Ok(img) = image::load_from_memory(&img_data) {
                            let rgba = img.into_rgba8();
                            let size = [rgba.width() as _, rgba.height() as _];
                            let pixels = rgba.into_raw();

                            let color_image =
                                egui::ColorImage::from_rgba_unmultiplied(size, &pixels);
                            let texture = ctx.load_texture(
                                &video.filename,
                                color_image,
                                egui::TextureOptions::default(),
                            );

                            self.thumbnails.insert(video.path.clone(), texture);
                        }
                    }
                }
            }
        }
    }

    fn generate_thumbnail(video_path: PathBuf, thumb_path: PathBuf) {
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            let _ = std::process::Command::new("ffmpeg")
                .args([
                    "-y",
                    "-i",
                    &video_path.to_string_lossy(),
                    "-ss",
                    "00:00:01",
                    "-vframes",
                    "1",
                    "-vf",
                    "scale=400:-1",
                    "-q:v",
                    "5",
                    &thumb_path.to_string_lossy(),
                ])
                .creation_flags(CREATE_NO_WINDOW)
                .spawn();
        }
    }

    pub fn update(&mut self, ctx: &egui::Context, _is_open: &mut bool) {
        if !self.loaded {
            self.scan_videos(ctx);
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            let total: usize = self.videos_by_game.iter().map(|(_, v)| v.len()).sum();
            ui.horizontal(|ui| {
                ui.heading("Video Gallery");
                ui.label(format!("({} videos, {} games)", total, self.videos_by_game.len()));
            });
            ui.separator();

            if self.videos_by_game.is_empty() {
                ui.vertical_centered(|ui| {
                    ui.add_space(50.0);
                    ui.label(egui::RichText::new("No videos found").size(18.0).weak());
                    ui.label(egui::RichText::new("Save some clips to see them here").size(14.0).weak());
                });
                return;
            }

            let tile_width = 200.0;
            let thumb_height = 120.0;
            let spacing = 10.0;
            let available_width = ui.available_width();
            let cols = ((available_width + spacing) / (tile_width + spacing)).floor() as usize;
            let cols = cols.max(1);

            let videos_by_game = self.videos_by_game.clone();
            let expanded_games = self.expanded_games.clone();
            let selected = self.selected.clone();
            let thumbnails = self.thumbnails.clone();
            let cache_dir = self.cache_directory.clone();
            let mut new_expanded = self.expanded_games.clone();
            let mut new_selected = self.selected.clone();
            let mut thumbs_to_generate: Vec<(PathBuf, PathBuf)> = Vec::new();

            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for (game, videos) in &videos_by_game {
                        let is_expanded = expanded_games.get(game).copied().unwrap_or(true);

                        ui.horizontal(|ui| {
                            let icon = if is_expanded { "▼" } else { "▶" };
                            let label = format!("{} {} ({} videos)", icon, game, videos.len());

                            if ui.button(&label).clicked() {
                                new_expanded.insert(game.clone(), !is_expanded);
                            }
                        });

                        if is_expanded {
                            ui.add_space(6.0);

                            let rows = videos.len().div_ceil(cols);

                            for row in 0..rows {
                                ui.horizontal(|ui| {
                                    for col in 0..cols {
                                        let idx = row * cols + col;
                                        if idx >= videos.len() {
                                            break;
                                        }

                                        let video = &videos[idx];
                                        let is_selected = selected == Some((game.clone(), idx));

                                        let response = ui.scope(|ui| {
                                            ui.set_min_width(tile_width);
                                            ui.set_max_width(tile_width);

                                            let mut hovered_tile = false;
                                            let bg_color = if is_selected {
                                                egui::Color32::from_rgb(40, 60, 90)
                                            } else {
                                                egui::Color32::from_rgb(35, 35, 40)
                                            };

                                            egui::Frame::default()
                                                .fill(bg_color)
                                                .corner_radius(egui::CornerRadius::same(6))
                                                .inner_margin(egui::Margin::same(8))
                                                .show(ui, |ui| {
                                                    ui.vertical(|ui| {
                                                        ui.set_width(tile_width - 16.0);
                                                        let thumb_size = egui::vec2(tile_width - 16.0, thumb_height);

                                                        if let Some(texture) = thumbnails.get(&video.path) {
                                                            let img_response = ui.add(
                                                                egui::Image::from_texture(texture)
                                                                    .fit_to_exact_size(thumb_size)
                                                                    .maintain_aspect_ratio(true)
                                                                    .sense(egui::Sense::click())
                                                            );

                                                            hovered_tile |= img_response.hovered();

                                                            if img_response.double_clicked() {
                                                                Self::open_video(video);
                                                            }
                                                        } else {
                                                            let (rect, response) = ui.allocate_exact_size(egui::vec2(tile_width - 16.0, thumb_height), egui::Sense::click());
                                                            ui.painter().rect_filled(rect, egui::CornerRadius::same(4), egui::Color32::from_rgb(25, 25, 30));
                                                            ui.painter().text(
                                                                rect.center(),
                                                                egui::Align2::CENTER_CENTER,
                                                                "▶",
                                                                egui::FontId::proportional(28.0),
                                                                egui::Color32::from_rgb(70, 70, 80),
                                                            );
                                                            use std::hash::{Hash, Hasher};
                                                            let mut hasher = std::collections::hash_map::DefaultHasher::new();
                                                            video.path.hash(&mut hasher);
                                                            let hash = hasher.finish();
                                                            let thumb_path = cache_dir.join(format!("{:016x}.jpg", hash));
                                                            thumbs_to_generate.push((video.path.clone(), thumb_path));

                                                            hovered_tile |= response.hovered();
                                                            if response.double_clicked() {
                                                                Self::open_video(video);
                                                            }
                                                        }

                                                        ui.add_space(6.0);

                                                        let display_name = if video.filename.len() > 25 {
                                                            format!("{}...", &video.filename[..22])
                                                        } else {
                                                            video.filename.clone()
                                                        };
                                                        ui.label(egui::RichText::new(display_name)
                                                            .size(11.0)
                                                            .strong()
                                                            .color(egui::Color32::from_rgb(220, 220, 220)));

                                                        ui.add_space(2.0);

                                                        ui.label(egui::RichText::new(format!("{:.1} MB", video.size_mb))
                                                            .size(10.0)
                                                            .color(egui::Color32::from_rgb(110, 110, 120)));
                                                    });
                                                    if hovered_tile && !is_selected {
                                                        let hover_rect = ui.min_rect();
                                                        ui.painter().rect_filled(
                                                            hover_rect,
                                                            egui::CornerRadius::same(6),
                                                            egui::Color32::from_rgba_unmultiplied(60, 90, 140, 30),
                                                        );
                                                    }
                                                });
                                        }).response;

                                        if response.clicked() {
                                            new_selected = Some((game.clone(), idx));
                                        }
                                        if response.double_clicked() {
                                            Self::open_video(video);
                                        }

                                        ui.add_space(spacing);
                                    }
                                });
                                ui.add_space(spacing);
                            }
                        }
                        ui.add_space(8.0);
                    }
                });

            self.expanded_games = new_expanded;
            self.selected = new_selected;

            for (video_path, thumb_path) in thumbs_to_generate {
                if !self.thumbnails_generating.contains(&video_path) {
                    self.thumbnails_generating.insert(video_path.clone());
                    let _ = std::fs::create_dir_all(&self.cache_directory);
                    Self::generate_thumbnail(video_path, thumb_path);
                }
            }
        });
    }

    fn open_video(video: &VideoEntry) {
        info!("Opening video: {:?}", video.path);

        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            if let Err(e) = std::process::Command::new("cmd")
                .args(["/C", "start", "", &video.path.to_string_lossy()])
                .creation_flags(CREATE_NO_WINDOW)
                .spawn()
            {
                error!("Failed to open video: {}", e);
            }
        }
    }

    pub fn refresh(&mut self) {
        self.loaded = false;
        self.videos_by_game.clear();
        self.thumbnails.clear();
        self.selected = None;
    }
}

impl eframe::App for GalleryApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let mut _dummy = true;
        self.update(ctx, &mut _dummy);
    }
}
