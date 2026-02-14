use eframe::egui;
use log::{debug, error, info, warn};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::recorder::{Recorder, RecorderState};
use crate::settings::{Framerate, HotkeyPreset, Quality, Resolution, VideoEncoder};

/// Which panel is visible in the GUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Panel {
    Main,
    Settings,
}

enum SaveResult {
    Success(PathBuf),
    Error(String),
}

enum StartResult {
    Success,
    Error(String),
}

/// The main GUI application — Medal-style compact overlay.
/// 
/// This struct manages the egui state, UI panels, and communication with 
/// the background [`Recorder`] via shared state and message passing.
pub struct LiteClipApp {
    /// Shared pointer to the recorder backend.
    pub recorder: Arc<Mutex<Recorder>>,
    /// Current status message shown in the UI.
    pub status_message: String,
    /// Countdown timer for the status message visibility.
    pub status_timer: f64,
    /// Whether a save operation is currently in progress.
    pub save_in_progress: bool,
    /// Whether a start operation is currently in progress.
    pub start_in_progress: bool,
    
    panel: Panel,
    /// Tracks pending hotkey change so we can signal main to re-register.
    pub pending_hotkey: Option<HotkeyPreset>,
    save_result_rx: Option<Receiver<SaveResult>>,
    start_result_rx: Option<Receiver<StartResult>>,
    visuals_initialized: bool,
    cached_state: RecorderState,
    cached_ffmpeg_found: bool,
    cached_last_saved_path: Option<PathBuf>,
    cached_hotkey_label: String,
}

impl LiteClipApp {
    /// Create a new LiteClipApp instance.
    /// 
    /// # Arguments
    /// * `recorder` - The shared recorder backend.
    pub fn new(recorder: Arc<Mutex<Recorder>>) -> Self {
        let (cached_state, cached_ffmpeg_found, cached_last_saved_path, cached_hotkey_label) = {
            let rec = recorder.lock().unwrap();
            (
                rec.state,
                rec.ffmpeg_found,
                rec.last_saved_path.clone(),
                rec.settings.hotkey.label().to_string(),
            )
        };

        let mut app = Self {
            recorder,
            status_message: String::new(),
            status_timer: 0.0,
            save_in_progress: false,
            start_in_progress: false,
            panel: Panel::Main,
            pending_hotkey: None,
            save_result_rx: None,
            start_result_rx: None,
            visuals_initialized: false,
            cached_state,
            cached_ffmpeg_found,
            cached_last_saved_path,
            cached_hotkey_label,
        };

        // Auto-start replay buffer on launch
        if cached_ffmpeg_found {
            info!("Auto-starting replay buffer on launch");
            app.trigger_start();
        }

        app
    }

    fn trigger_start(&mut self) {
        if self.start_in_progress {
            warn!("Start already in progress — ignoring duplicate trigger");
            return;
        }

        info!("User triggered: START recording");
        self.start_in_progress = true;
        self.status_message = "Starting replay buffer...".to_string();
        self.status_timer = 2.0;

        let recorder = Arc::clone(&self.recorder);
        let (tx, rx) = mpsc::channel();
        self.start_result_rx = Some(rx);

        std::thread::spawn(move || {
            let result = {
                let mut rec = recorder.lock().unwrap();
                rec.start()
            };
            let message = match result {
                Ok(()) => StartResult::Success,
                Err(err) => StartResult::Error(err),
            };
            let _ = tx.send(message);
        });
    }

    fn poll_start_result(&mut self) {
        let Some(rx) = self.start_result_rx.as_ref() else {
            return;
        };

        match rx.try_recv() {
            Ok(StartResult::Success) => {
                info!("Recording started successfully");
                self.status_message = "Replay buffer active!".into();
                self.status_timer = 3.0;
                self.start_in_progress = false;
                self.start_result_rx = None;
            }
            Ok(StartResult::Error(e)) => {
                error!("Start failed: {}", e);
                self.status_message = format!("Error: {}", e);
                self.status_timer = 5.0;
                self.start_in_progress = false;
                self.start_result_rx = None;
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                error!("Start worker thread disconnected unexpectedly");
                self.status_message = "Start worker disconnected.".to_string();
                self.status_timer = 5.0;
                self.start_in_progress = false;
                self.start_result_rx = None;
            }
        }
    }

    fn refresh_main_snapshot(&mut self) {
        if let Ok(rec) = self.recorder.try_lock() {
            self.cached_state = rec.state;
            self.cached_ffmpeg_found = rec.ffmpeg_found;
            self.cached_last_saved_path = rec.last_saved_path.clone();
            self.cached_hotkey_label = rec.settings.hotkey.label().to_string();
        }
    }

    /// Trigger an auto-save clip (no dialog — saves to ~/Videos/LiteClip/).
    /// 
    /// This spawns a background thread to handle segment concatenation.
    pub fn trigger_save(&mut self) {
        if self.save_in_progress {
            warn!("Save already in progress — ignoring duplicate trigger");
            return;
        }

        info!("User triggered: SAVE clip");
        self.save_in_progress = true;
        self.status_message = "Saving clip...".to_string();
        self.status_timer = 2.0;

        let recorder = Arc::clone(&self.recorder);
        let (tx, rx) = mpsc::channel();
        self.save_result_rx = Some(rx);

        std::thread::spawn(move || {
            let result = {
                let mut rec = recorder.lock().unwrap();
                rec.save_clip_auto()
            };
            let message = match result {
                Ok(path) => SaveResult::Success(path),
                Err(err) => SaveResult::Error(err),
            };
            let _ = tx.send(message);
        });
    }

    fn poll_save_result(&mut self) {
        let Some(rx) = self.save_result_rx.as_ref() else {
            return;
        };

        match rx.try_recv() {
            Ok(SaveResult::Success(path)) => {
                let name = path
                    .file_name()
                    .unwrap_or_default()
                    .to_str()
                    .unwrap_or("clip.mp4");
                info!("Clip saved: {}", path.display());
                self.status_message = format!("Clip saved! {}", name);
                self.status_timer = 5.0;
                self.save_in_progress = false;
                self.save_result_rx = None;
            }
            Ok(SaveResult::Error(e)) => {
                error!("Save failed: {}", e);
                self.status_message = format!("Save failed: {}", e);
                self.status_timer = 5.0;
                self.save_in_progress = false;
                self.save_result_rx = None;
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                error!("Save worker thread disconnected unexpectedly");
                self.status_message = "Save worker disconnected.".to_string();
                self.status_timer = 5.0;
                self.save_in_progress = false;
                self.save_result_rx = None;
            }
        }
    }

    /// Draw the main panel (Medal-style).
    fn draw_main(&mut self, ui: &mut egui::Ui) {
        self.refresh_main_snapshot();
        let state = self.cached_state;
        let ffmpeg_found = self.cached_ffmpeg_found;
        let last_saved = self.cached_last_saved_path.clone();
        let hotkey_label = self.cached_hotkey_label.clone();

        // --- FFmpeg warning ---
        if !ffmpeg_found {
            ui.vertical_centered(|ui| {
                ui.add_space(12.0);
                ui.label(
                    egui::RichText::new("⚠  FFmpeg Not Found")
                        .size(16.0)
                        .color(egui::Color32::from_rgb(255, 80, 80))
                        .strong(),
                );
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(
                        "Install FFmpeg and add it to PATH,\nthen restart LiteClip.",
                    )
                    .color(egui::Color32::from_gray(120)),
                );
            });
            return;
        }

        // --- Big status area ---
        ui.vertical_centered(|ui| {
            match state {
                RecorderState::Idle => {
                    ui.label(
                        egui::RichText::new("STANDBY")
                            .size(16.0)
                            .color(egui::Color32::from_gray(80))
                            .strong(),
                    );
                }
                RecorderState::Recording => {
                    // Pulsing red dot effect
                    let pulse = ((ui.input(|i| i.time) * 3.0).sin() * 0.5 + 0.5) as u8;
                    let alpha = 100 + pulse.saturating_mul(155);
                    ui.label(
                        egui::RichText::new("● REC")
                            .size(16.0)
                            .color(egui::Color32::from_rgba_premultiplied(255, 50, 50, alpha))
                            .strong(),
                    );
                }
                RecorderState::Saving => {
                    ui.label(
                        egui::RichText::new("SAVING...")
                            .size(16.0)
                            .color(egui::Color32::WHITE)
                            .strong(),
                    );
                }
            }
        });

        ui.add_space(6.0);

        // --- Control buttons ---
        ui.horizontal(|ui| {
            let avail = ui.available_width();
            let btn_w = (avail - 12.0) / 2.0;
            let btn_size = egui::vec2(btn_w, 36.0);

            match state {
                RecorderState::Idle => {
                    let start_btn = ui.add_enabled(
                        !self.start_in_progress,
                        egui::Button::new(
                            egui::RichText::new(if self.start_in_progress {
                                "INITIALIZING..."
                            } else {
                                "START BUFFER"
                            })
                            .size(14.0),
                        )
                        .fill(egui::Color32::from_gray(10))
                        .stroke(egui::Stroke::new(1.0, egui::Color32::from_gray(60)))
                        .min_size(egui::vec2(avail, 36.0)),
                    );
                    if start_btn.clicked() {
                        self.trigger_start();
                    }
                }
                RecorderState::Recording => {
                    // Save clip button (prominent)
                    if ui
                        .add_sized(
                            btn_size,
                            egui::Button::new(
                                egui::RichText::new(format!("SAVE CLIP [{}]", hotkey_label))
                                    .size(13.0)
                                    .color(egui::Color32::BLACK), // Black text on white/light button
                            )
                            .fill(egui::Color32::from_gray(220)) // Light button
                            .corner_radius(2.0),
                        )
                        .clicked()
                    {
                        self.trigger_save();
                    }

                    // Stop button
                    if ui
                        .add_sized(
                            btn_size,
                            egui::Button::new(egui::RichText::new("STOP").size(13.0).color(egui::Color32::from_rgb(255, 80, 80)))
                                .fill(egui::Color32::BLACK)
                                .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(100, 40, 40)))
                                .corner_radius(2.0),
                        )
                        .clicked()
                    {
                        info!("User triggered: STOP recording");
                        let stopped = if let Ok(mut rec) = self.recorder.try_lock() {
                            rec.stop();
                            true
                        } else {
                            false
                        };
                        if stopped {
                            info!("Recording stopped by user");
                            self.status_message = "Buffer stopped.".into();
                            self.status_timer = 3.0;
                            self.refresh_main_snapshot();
                        } else {
                            warn!("Could not stop — recorder lock is held");
                            self.status_message = "Recorder is busy, try again...".into();
                            self.status_timer = 2.0;
                        }
                    }
                }
                RecorderState::Saving => {
                    ui.add_enabled(
                        false,
                        egui::Button::new(egui::RichText::new("Saving...").size(14.0))
                            .frame(false)
                            .min_size(egui::vec2(avail, 36.0)),
                    );
                }
            }
        });

        // --- Status toast ---
        if self.status_timer > 0.0 {
            ui.add_space(4.0);
            let alpha = (self.status_timer.min(1.0) * 255.0) as u8;
            ui.label(
                egui::RichText::new(&self.status_message)
                    .small()
                    .color(egui::Color32::from_rgba_premultiplied(180, 210, 240, alpha)),
            );
        }

        // --- Footer: last save + settings gear ---
        ui.add_space(2.0);
        ui.separator();
        ui.horizontal(|ui| {
            if let Some(ref path) = last_saved {
                let name = path.file_name().unwrap_or_default().to_str().unwrap_or("");
                ui.label(
                    egui::RichText::new(format!("LAST: {}", name))
                        .size(10.0)
                        .color(egui::Color32::from_gray(100)),
                );
            } else {
                ui.label(
                    egui::RichText::new("NO CLIPS")
                        .size(10.0)
                        .color(egui::Color32::from_gray(60)),
                );
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(
                        egui::Button::new(
                            egui::RichText::new("SETTINGS")
                                .size(10.0)
                                .color(egui::Color32::from_gray(120)),
                        )
                        .frame(false),
                    )
                    .clicked()
                {
                    self.panel = Panel::Settings;
                }
            });
        });
    }

    /// Draw the settings panel.
    fn draw_settings(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui
                .add(
                    egui::Button::new(
                        egui::RichText::new("< BACK").color(egui::Color32::from_gray(120)),
                    )
                    .frame(false),
                )
                .clicked()
            {
                self.panel = Panel::Main;
            }
            ui.heading(
                egui::RichText::new("SETTINGS")
                    .color(egui::Color32::WHITE)
                    .strong(),
            );
        });

        ui.separator();

        // Snapshot current settings to avoid holding lock across closures
        let (
            mut hotkey,
            mut buffer_seconds,
            mut quality,
            mut video_encoder,
            mut framerate,
            mut resolution,
            mut capture_audio,
            mut audio_device,
            audio_devices,
            available_video_encoders,
            output_dir,
        ) = {
            let rec = self.recorder.lock().unwrap();
            (
                rec.settings.hotkey,
                rec.settings.buffer_seconds,
                rec.settings.quality,
                rec.settings.video_encoder,
                rec.settings.framerate,
                rec.settings.resolution,
                rec.settings.capture_audio,
                rec.settings.audio_device.clone(),
                rec.audio_devices.clone(),
                rec.video_encoders.clone(),
                rec.settings.output_dir.clone(),
            )
        };

        let original_hotkey = hotkey;

        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.spacing_mut().item_spacing = egui::vec2(8.0, 6.0);

            // --- Hotkey ---
            setting_row(ui, "Save Hotkey", |ui| {
                egui::ComboBox::from_id_salt("hotkey_sel")
                    .selected_text(hotkey.label())
                    .width(110.0)
                    .show_ui(ui, |ui| {
                        for hk in HotkeyPreset::all() {
                            ui.selectable_value(&mut hotkey, *hk, hk.label());
                        }
                    });
            });

            // --- Buffer Length ---
            setting_row(ui, "Buffer Length", |ui| {
                egui::ComboBox::from_id_salt("buf_len")
                    .selected_text(format_buffer_length(buffer_seconds))
                    .width(110.0)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut buffer_seconds, 30, "30 seconds");
                        ui.selectable_value(&mut buffer_seconds, 60, "1 minute");
                        ui.selectable_value(&mut buffer_seconds, 120, "2 minutes");
                        ui.selectable_value(&mut buffer_seconds, 300, "5 minutes");
                        ui.selectable_value(&mut buffer_seconds, 600, "10 minutes");
                    });
            });

            ui.add_space(2.0);
            section_header(ui, "VIDEO");

            // --- Quality ---
            setting_row(ui, "Quality", |ui| {
                egui::ComboBox::from_id_salt("quality_sel")
                    .selected_text(quality.label())
                    .width(150.0)
                    .show_ui(ui, |ui| {
                        for q in Quality::all() {
                            ui.selectable_value(&mut quality, *q, q.label());
                        }
                    });
            });

            // --- Encoder ---
            setting_row(ui, "Encoder", |ui| {
                egui::ComboBox::from_id_salt("encoder_sel")
                    .selected_text(video_encoder.label())
                    .width(150.0)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut video_encoder,
                            VideoEncoder::Auto,
                            VideoEncoder::Auto.label(),
                        );

                        for enc in VideoEncoder::all() {
                            if *enc == VideoEncoder::Auto {
                                continue;
                            }
                            if *enc == VideoEncoder::Libx264
                                || available_video_encoders.contains(enc)
                            {
                                ui.selectable_value(&mut video_encoder, *enc, enc.label());
                            }
                        }
                    });
            });

            // --- Framerate ---
            setting_row(ui, "Framerate", |ui| {
                egui::ComboBox::from_id_salt("fps_sel")
                    .selected_text(framerate.label())
                    .width(110.0)
                    .show_ui(ui, |ui| {
                        for f in Framerate::all() {
                            ui.selectable_value(&mut framerate, *f, f.label());
                        }
                    });
            });

            // --- Resolution ---
            setting_row(ui, "Resolution", |ui| {
                egui::ComboBox::from_id_salt("res_sel")
                    .selected_text(resolution.label())
                    .width(110.0)
                    .show_ui(ui, |ui| {
                        for r in Resolution::all() {
                            ui.selectable_value(&mut resolution, *r, r.label());
                        }
                    });
            });

            ui.add_space(2.0);
            section_header(ui, "AUDIO");

            // --- Audio toggle ---
            setting_row(ui, "Capture Audio", |ui| {
                ui.checkbox(&mut capture_audio, "");
            });

            // --- Audio device selector ---
            if capture_audio {
                setting_row(ui, "Audio Device", |ui| {
                    let mut current = audio_device.clone().unwrap_or_else(|| "None".to_string());

                    egui::ComboBox::from_id_salt("audio_dev")
                        .selected_text(truncate_str(&current, 20))
                        .width(150.0)
                        .show_ui(ui, |ui| {
                            for dev in &audio_devices {
                                ui.selectable_value(&mut current, dev.clone(), dev.as_str());
                            }
                            if audio_devices.is_empty() {
                                ui.label(
                                    egui::RichText::new("No devices detected")
                                        .small()
                                        .color(egui::Color32::from_rgb(160, 100, 100)),
                                );
                            }
                        });

                    audio_device = if current == "None" {
                        None
                    } else {
                        Some(current)
                    };
                });
            }

            ui.add_space(2.0);
            section_header(ui, "OUTPUT");

            // --- Output directory ---
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("Save Folder")
                        .color(egui::Color32::from_rgb(180, 190, 210)),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add(
                            egui::Button::new("BROWSE")
                                .fill(egui::Color32::from_gray(20))
                                .stroke(egui::Stroke::new(1.0, egui::Color32::from_gray(40)))
                                .corner_radius(2.0),
                        )
                        .clicked()
                    {
                        if let Some(dir) = rfd::FileDialog::new()
                            .set_title("Choose Output Folder")
                            .set_directory(&output_dir)
                            .pick_folder()
                        {
                            let mut rec = self.recorder.lock().unwrap();
                            rec.settings.output_dir = dir;
                            rec.settings.save();
                            info!("Output directory saved");
                        }
                    }
                });
            });

            // Show current path
            ui.label(
                egui::RichText::new(output_dir.display().to_string())
                    .small()
                    .monospace()
                    .color(egui::Color32::from_rgb(120, 130, 150)),
            );

            // --- Open output folder ---
            ui.add_space(4.0);
            if ui
                .add(
                    egui::Button::new(
                        egui::RichText::new("OPEN CLIPS FOLDER")
                            .color(egui::Color32::from_gray(200)),
                    )
                    .fill(egui::Color32::from_gray(20))
                    .stroke(egui::Stroke::new(1.0, egui::Color32::from_gray(40)))
                    .corner_radius(2.0),
                )
                .clicked()
            {
                let dir = output_dir.clone();
                let _ = std::fs::create_dir_all(&dir);
                let _ = std::process::Command::new("explorer").arg(dir).spawn();
            }
        });

        // Write all settings back to the recorder
        {
            let mut rec = self.recorder.lock().unwrap();
            
            // detecting changes to avoid unnecessary saves (optional optimization, but good practice)
            let changed = rec.settings.hotkey != hotkey
                || rec.settings.buffer_seconds != buffer_seconds
                || rec.settings.quality != quality
                || rec.settings.video_encoder != video_encoder
                || rec.settings.framerate != framerate
                || rec.settings.resolution != resolution
                || rec.settings.capture_audio != capture_audio
                || rec.settings.audio_device != audio_device;

            rec.settings.hotkey = hotkey;
            rec.settings.buffer_seconds = buffer_seconds;
            rec.settings.quality = quality;
            rec.settings.video_encoder = video_encoder;
            rec.settings.framerate = framerate;
            rec.settings.resolution = resolution;
            rec.settings.capture_audio = capture_audio;
            rec.settings.audio_device = audio_device;
            
            if changed {
                let settings_to_save = rec.settings.clone();
                std::thread::spawn(move || {
                    settings_to_save.save();
                    info!("Settings saved to disk (background)");
                });
            }
        }

        // Signal hotkey change if needed
        if hotkey != original_hotkey {
            self.pending_hotkey = Some(hotkey);
        }
    }
}

impl eframe::App for LiteClipApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_start_result();
        self.poll_save_result();
        self.refresh_main_snapshot();

        let dt = ctx.input(|i| i.stable_dt) as f64;

        // Decrease status timer
        if self.status_timer > 0.0 {
            self.status_timer = (self.status_timer - dt).max(0.0);
        }

        if !self.visuals_initialized {
            let mut visuals = egui::Visuals::dark();
            visuals.override_text_color = Some(egui::Color32::from_gray(230));
            visuals.window_fill = egui::Color32::BLACK;
            visuals.panel_fill = egui::Color32::BLACK;
            
            visuals.widgets.noninteractive.bg_fill = egui::Color32::BLACK;
            visuals.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, egui::Color32::from_gray(30));
            visuals.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, egui::Color32::from_gray(180));

            visuals.widgets.inactive.bg_fill = egui::Color32::from_gray(10);
            visuals.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, egui::Color32::from_gray(180));
            
            visuals.widgets.hovered.bg_fill = egui::Color32::from_gray(25);
            visuals.widgets.hovered.fg_stroke = egui::Stroke::new(1.0, egui::Color32::WHITE);
            
            visuals.widgets.active.bg_fill = egui::Color32::from_gray(40);
            visuals.widgets.active.fg_stroke = egui::Stroke::new(1.0, egui::Color32::WHITE);
            
            visuals.selection.bg_fill = egui::Color32::from_gray(50);
            visuals.selection.stroke = egui::Stroke::new(1.0, egui::Color32::WHITE);
            
            // Thin borders
            visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, egui::Color32::from_gray(30));
            visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, egui::Color32::from_gray(60));
            visuals.widgets.active.bg_stroke = egui::Stroke::new(1.0, egui::Color32::WHITE);

            ctx.set_visuals(visuals);
            self.visuals_initialized = true;
        }

        let should_animate = self.status_timer > 0.0
            || self.start_in_progress
            || self.save_in_progress
            || matches!(
                self.cached_state,
                RecorderState::Recording | RecorderState::Saving
            );
        if should_animate {
            ctx.request_repaint_after(Duration::from_millis(500));
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::central_panel(&ctx.style()).inner_margin(12.0))
            .show(ctx, |ui| {
                // --- Header ---
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("LITECLIP")
                            .size(16.0)
                            .color(egui::Color32::WHITE)
                            .strong(),
                    );
                });

                ui.add_space(8.0);

                match self.panel {
                    Panel::Main => self.draw_main(ui),
                    Panel::Settings => self.draw_settings(ui),
                }
            });
    }
}

// ── Helpers ────────────────────────────────────────────────────

/// A standard settings row: label on the left, widget on the right.
fn setting_row(ui: &mut egui::Ui, label: &str, add_widget: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(label.to_uppercase()).size(10.0).color(egui::Color32::from_gray(120)));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), add_widget);
    });
}

/// Section header divider.
fn section_header(ui: &mut egui::Ui, title: &str) {
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(title)
            .size(11.0)
            .color(egui::Color32::WHITE)
            .strong(),
    );
    ui.separator();
}

/// Format buffer length for display.
fn format_buffer_length(secs: u64) -> String {
    if secs < 60 {
        format!("{} seconds", secs)
    } else if secs < 600 {
        format!(
            "{} minute{}",
            secs / 60,
            if secs / 60 > 1 { "s" } else { "" }
        )
    } else {
        format!("{} minutes", secs / 60)
    }
}

/// Truncate a string for display.
fn truncate_str(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}…", &s[..max_len - 1])
    }
}
