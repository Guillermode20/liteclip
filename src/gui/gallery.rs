use eframe::egui;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;
use tokio::sync::mpsc::Sender;
use tracing::{debug, error, info};

use crate::config::Config;
use crate::platform::AppEvent;

pub fn show_gallery_gui(event_tx: Sender<AppEvent>) {
    crate::gui::manager::send_gui_message(crate::gui::manager::GuiMessage::ShowGallery(event_tx));
}

#[derive(Clone)]
struct VideoEntry {
    path: PathBuf,
    filename: String,
    folder: String,
    size_mb: f64,
    modified: SystemTime,
    thumbnail_path: Option<PathBuf>,
    thumbnail_loaded: bool,
}

pub struct GalleryApp {
    event_tx: Sender<AppEvent>,
    save_directory: PathBuf,
    videos: Vec<VideoEntry>,
    selected: Option<usize>,
    loaded: bool,
    thumbnails: HashMap<PathBuf, egui::TextureHandle>,
    loading_thumbnails: bool,
    scroll_offset: f32,
}

impl GalleryApp {
    pub fn new(config: &Config, event_tx: Sender<AppEvent>) -> Self {
        Self {
            event_tx,
            save_directory: PathBuf::from(&config.general.save_directory),
            videos: Vec::new(),
            selected: None,
            loaded: false,
            thumbnails: HashMap::new(),
            loading_thumbnails: false,
            scroll_offset: 0.0,
        }
    }

    fn scan_videos(&mut self, ctx: &egui::Context) {
        info!("Scanning videos in: {:?}", self.save_directory);
        self.videos.clear();

        if !self.save_directory.exists() {
            debug!("Save directory does not exist yet");
            self.loaded = true;
            return;
        }

        let mut entries = Vec::new();

        fn scan_dir(dir: &PathBuf, entries: &mut Vec<VideoEntry>, base_dir: &PathBuf) {
            if let Ok(read_dir) = std::fs::read_dir(dir) {
                for entry in read_dir.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        scan_dir(&path, entries, base_dir);
                    } else if path.extension().map(|e| e == "mp4").unwrap_or(false) {
                        if let Ok(metadata) = entry.metadata() {
                            let filename = path
                                .file_name()
                                .map(|n| n.to_string_lossy().to_string())
                                .unwrap_or_default();

                            let relative = path.strip_prefix(base_dir).unwrap_or(&path);
                            let folder = relative
                                .parent()
                                .map(|p| p.to_string_lossy().to_string())
                                .unwrap_or_default();

                            let size_mb = metadata.len() as f64 / (1024.0 * 1024.0);
                            let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);

                            let thumb_path = path.with_extension("thumb.jpg");

                            entries.push(VideoEntry {
                                path,
                                filename,
                                folder,
                                size_mb,
                                modified,
                                thumbnail_path: Some(thumb_path).filter(|p| p.exists()),
                                thumbnail_loaded: false,
                            });
                        }
                    }
                }
            }
        }

        scan_dir(&self.save_directory, &mut entries, &self.save_directory);

        entries.sort_by(|a, b| b.modified.cmp(&a.modified));

        self.videos = entries;
        self.loaded = true;
        info!("Found {} videos", self.videos.len());

        self.load_thumbnails(ctx);
    }

    fn load_thumbnails(&mut self, ctx: &egui::Context) {
        if self.loading_thumbnails {
            return;
        }
        self.loading_thumbnails = true;

        for video in &mut self.videos {
            if video.thumbnail_loaded {
                continue;
            }

            if let Some(thumb_path) = &video.thumbnail_path {
                if let Ok(img_data) = std::fs::read(thumb_path) {
                    if let Ok(img) = image::load_from_memory(&img_data) {
                        let rgba = img.into_rgba8();
                        let size = [rgba.width() as _, rgba.height() as _];
                        let pixels = rgba.into_raw();

                        let color_image = egui::ColorImage::from_rgba_unmultiplied(size, &pixels);
                        let texture = ctx.load_texture(
                            &video.filename,
                            color_image,
                            egui::TextureOptions::default(),
                        );

                        self.thumbnails.insert(video.path.clone(), texture);
                        video.thumbnail_loaded = true;
                    }
                }
            }
        }

        self.loading_thumbnails = false;
    }

    pub fn update(&mut self, ctx: &egui::Context, _is_open: &mut bool) {
        if !self.loaded {
            self.scan_videos(ctx);
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Video Gallery");
                ui.label(format!("({} videos)", self.videos.len()));
            });
            ui.separator();

            if self.videos.is_empty() {
                ui.vertical_centered(|ui| {
                    ui.add_space(50.0);
                    ui.label(egui::RichText::new("No videos found").size(18.0).weak());
                    ui.label(
                        egui::RichText::new("Save some clips to see them here")
                            .size(14.0)
                            .weak(),
                    );
                });
            } else {
                let available_width = ui.available_width();
                let tile_width = 200.0;
                let tile_height = 150.0;
                let spacing = 10.0;
                let cols = ((available_width + spacing) / (tile_width + spacing)).floor() as usize;
                let cols = cols.max(1);

                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let rows = (self.videos.len() + cols - 1) / cols;

                        for row in 0..rows {
                            ui.horizontal(|ui| {
                                for col in 0..cols {
                                    let idx = row * cols + col;
                                    if idx >= self.videos.len() {
                                        break;
                                    }

                                    let video = &self.videos[idx];
                                    let is_selected = self.selected == Some(idx);

                                    let mut frame = egui::Frame::default()
                                        .inner_margin(egui::Margin::same(5.0))
                                        .outer_margin(egui::Margin::same(2.0))
                                        .corner_radius(egui::CornerRadius::same(8));

                                    if is_selected {
                                        frame = frame.fill(egui::Color32::from_rgb(40, 60, 80));
                                    }

                                    let response = frame
                                        .show(ui, |ui| {
                                            ui.set_min_width(tile_width);
                                            ui.set_max_width(tile_width);

                                            let (rect, _) = ui.allocate_exact_size(
                                                egui::vec2(tile_width - 10.0, 90.0),
                                                egui::Sense::click(),
                                            );

                                            if let Some(texture) = self.thumbnails.get(&video.path)
                                            {
                                                let mut job = egui::epaint::MeshJob::default();
                                                job.texture_id = texture.id();
                                                ui.painter().add(job);
                                                ui.image(texture);
                                            } else {
                                                ui.allocate_ui_with_layout(
                                                    egui::vec2(tile_width - 10.0, 90.0),
                                                    egui::Layout::centered_and_justified(
                                                        egui::Direction::TopDown,
                                                    ),
                                                    |ui| {
                                                        ui.label(
                                                            egui::RichText::new("▶")
                                                                .size(30.0)
                                                                .weak(),
                                                        );
                                                    },
                                                );
                                            }

                                            ui.add_space(4.0);

                                            ui.with_layout(
                                                egui::Layout::left_to_right(egui::Align::TOP)
                                                    .with_main_wrap(true),
                                                |ui| {
                                                    let display_name = if video.filename.len() > 20
                                                    {
                                                        format!("{}...", &video.filename[..17])
                                                    } else {
                                                        video.filename.clone()
                                                    };
                                                    ui.label(
                                                        egui::RichText::new(display_name)
                                                            .size(11.0)
                                                            .strong(),
                                                    );
                                                },
                                            );

                                            ui.horizontal(|ui| {
                                                ui.label(
                                                    egui::RichText::new(format!(
                                                        "{:.1} MB",
                                                        video.size_mb
                                                    ))
                                                    .size(10.0)
                                                    .weak(),
                                                );
                                                if !video.folder.is_empty() {
                                                    ui.label(
                                                        egui::RichText::new(format!(
                                                            "• {}",
                                                            video.folder
                                                        ))
                                                        .size(10.0)
                                                        .weak(),
                                                    );
                                                }
                                            });
                                        })
                                        .response;

                                    if response.clicked() {
                                        self.selected = Some(idx);
                                    }

                                    if response.double_clicked() {
                                        self.open_video(idx);
                                    }

                                    if response.secondary_clicked() {
                                        self.show_context_menu(idx, ui);
                                    }
                                }
                            });
                            ui.add_space(spacing);
                        }
                    });
            }
        });
    }

    fn open_video(&self, idx: usize) {
        if idx >= self.videos.len() {
            return;
        }

        let video = &self.videos[idx];
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

        #[cfg(not(target_os = "windows"))]
        {
            if let Err(e) = open::that(&video.path) {
                error!("Failed to open video: {}", e);
            }
        }
    }

    fn show_context_menu(&self, idx: usize, ui: &mut egui::Ui) {
        if idx >= self.videos.len() {
            return;
        }

        let video = &self.videos[idx];

        egui::widgets::popup::popup_above_or_below_widget(
            ui,
            egui::Id::new("video_context_menu"),
            ui.next_widget_position(),
            egui::AboveOrBelow::Below,
            |ui| {
                ui.set_min_width(150.0);

                if ui.button("Open").clicked() {
                    self.open_video(idx);
                    ui.close_menu();
                }

                if ui.button("Open Folder").clicked() {
                    if let Some(parent) = video.path.parent() {
                        #[cfg(target_os = "windows")]
                        {
                            use std::os::windows::process::CommandExt;
                            const CREATE_NO_WINDOW: u32 = 0x08000000;
                            let _ = std::process::Command::new("explorer")
                                .arg(parent)
                                .creation_flags(CREATE_NO_WINDOW)
                                .spawn();
                        }
                    }
                    ui.close_menu();
                }

                ui.separator();

                if ui.button("Delete").clicked() {
                    if let Err(e) = std::fs::remove_file(&video.path) {
                        error!("Failed to delete video: {}", e);
                    }
                    if let Some(thumb) = &video.thumbnail_path {
                        let _ = std::fs::remove_file(thumb);
                    }
                    ui.close_menu();
                }
            },
            egui::PopupCloseBehavior::CloseOnClickOutside,
        );
    }

    pub fn refresh(&mut self) {
        self.loaded = false;
        self.videos.clear();
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
