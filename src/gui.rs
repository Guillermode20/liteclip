use eframe::egui;
use std::sync::{Arc, Mutex};

use crate::recorder::{Recorder, RecorderState};
use crate::settings::{Framerate, HotkeyPreset, Quality, Resolution};

/// Which panel is visible in the GUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Panel {
    Main,
    Settings,
}

/// The main GUI application — Medal-style compact overlay.
pub struct LiteClipApp {
    pub recorder: Arc<Mutex<Recorder>>,
    pub status_message: String,
    pub status_timer: f64,
    pub save_in_progress: bool,
    panel: Panel,
    /// Tracks pending hotkey change so we can signal main to re-register.
    pub pending_hotkey: Option<HotkeyPreset>,
}

impl LiteClipApp {
    pub fn new(recorder: Arc<Mutex<Recorder>>) -> Self {
        Self {
            recorder,
            status_message: String::new(),
            status_timer: 0.0,
            save_in_progress: false,
            panel: Panel::Main,
            pending_hotkey: None,
        }
    }

    /// Trigger an auto-save clip (no dialog — saves to ~/Videos/LiteClip/).
    pub fn trigger_save(&mut self) {
        if self.save_in_progress {
            return;
        }

        self.save_in_progress = true;
        let mut rec = self.recorder.lock().unwrap();
        match rec.save_clip_auto() {
            Ok(path) => {
                let name = path
                    .file_name()
                    .unwrap_or_default()
                    .to_str()
                    .unwrap_or("clip.mp4");
                self.status_message = format!("Clip saved! {}", name);
                self.status_timer = 5.0;
            }
            Err(e) => {
                self.status_message = format!("Save failed: {}", e);
                self.status_timer = 5.0;
            }
        }
        self.save_in_progress = false;
    }

    /// Draw the main panel (Medal-style).
    fn draw_main(&mut self, ui: &mut egui::Ui) {
        let (state, _elapsed, ffmpeg_found, _buffer_secs, last_saved, hotkey_label) = {
            let rec = self.recorder.lock().unwrap();
            (
                rec.state,
                rec.elapsed_seconds(),
                rec.ffmpeg_found,
                rec.settings.buffer_seconds,
                rec.last_saved_path.clone(),
                rec.settings.hotkey.label().to_string(),
            )
        };

        // --- FFmpeg warning ---
        if !ffmpeg_found {
            ui.vertical_centered(|ui| {
                ui.add_space(12.0);
                ui.label(
                    egui::RichText::new("⚠  FFmpeg Not Found")
                        .size(16.0)
                        .color(egui::Color32::from_rgb(255, 160, 60))
                        .strong(),
                );
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(
                        "Install FFmpeg and add it to PATH,\nthen restart LiteClip.",
                    )
                    .color(egui::Color32::from_rgb(160, 160, 170)),
                );
            });
            return;
        }

        // --- Big status area ---
        ui.vertical_centered(|ui| {
            match state {
                RecorderState::Idle => {
                    ui.label(
                        egui::RichText::new("⏸  STANDBY")
                            .size(18.0)
                            .color(egui::Color32::from_rgb(100, 100, 120))
                            .strong(),
                    );
                }
                RecorderState::Recording => {
                    // Pulsing red dot effect
                    let pulse = ((ui.input(|i| i.time) * 2.0).sin() * 0.5 + 0.5) as u8;
                    let red = 180 + pulse / 3;
                    ui.label(
                        egui::RichText::new("●  REC")
                            .size(20.0)
                            .color(egui::Color32::from_rgb(red, 50, 50))
                            .strong(),
                    );
                }
                RecorderState::Saving => {
                    ui.label(
                        egui::RichText::new("⏳  SAVING...")
                            .size(18.0)
                            .color(egui::Color32::from_rgb(255, 200, 60))
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
                    if ui
                        .add_sized(
                            egui::vec2(avail, 36.0),
                            egui::Button::new(
                                egui::RichText::new("▶  START BUFFER").size(14.0).strong(),
                            )
                            .fill(egui::Color32::from_rgb(35, 110, 65))
                            .corner_radius(6.0),
                        )
                        .clicked()
                    {
                        let mut rec = self.recorder.lock().unwrap();
                        match rec.start() {
                            Ok(()) => {
                                self.status_message = "Replay buffer active!".into();
                                self.status_timer = 3.0;
                            }
                            Err(e) => {
                                self.status_message = format!("Error: {}", e);
                                self.status_timer = 5.0;
                            }
                        }
                    }
                }
                RecorderState::Recording => {
                    // Save clip button (prominent)
                    if ui
                        .add_sized(
                            btn_size,
                            egui::Button::new(
                                egui::RichText::new(format!("💾  CLIP  [{}]", hotkey_label))
                                    .size(13.0)
                                    .strong(),
                            )
                            .fill(egui::Color32::from_rgb(50, 80, 160))
                            .corner_radius(6.0),
                        )
                        .clicked()
                    {
                        self.trigger_save();
                    }

                    // Stop button
                    if ui
                        .add_sized(
                            btn_size,
                            egui::Button::new(egui::RichText::new("⏹  STOP").size(13.0).strong())
                                .fill(egui::Color32::from_rgb(110, 35, 35))
                                .corner_radius(6.0),
                        )
                        .clicked()
                    {
                        let mut rec = self.recorder.lock().unwrap();
                        rec.stop();
                        self.status_message = "Buffer stopped.".into();
                        self.status_timer = 3.0;
                    }
                }
                RecorderState::Saving => {
                    ui.add_enabled(
                        false,
                        egui::Button::new(egui::RichText::new("Saving clip...").size(14.0))
                            .min_size(egui::vec2(avail, 36.0))
                            .corner_radius(6.0),
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
                    egui::RichText::new(format!("📁 {}", name))
                        .small()
                        .color(egui::Color32::from_rgb(120, 140, 160)),
                );
            } else {
                ui.label(
                    egui::RichText::new("No clips yet")
                        .small()
                        .color(egui::Color32::from_rgb(80, 80, 100)),
                );
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(
                        egui::Button::new(
                            egui::RichText::new("⚙")
                                .size(16.0)
                                .color(egui::Color32::from_rgb(160, 170, 190)),
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
                        egui::RichText::new("← Back").color(egui::Color32::from_rgb(140, 170, 220)),
                    )
                    .frame(false),
                )
                .clicked()
            {
                self.panel = Panel::Main;
            }
            ui.heading(
                egui::RichText::new("Settings")
                    .color(egui::Color32::from_rgb(200, 210, 230))
                    .strong(),
            );
        });

        ui.separator();

        // Snapshot current settings to avoid holding lock across closures
        let (
            mut hotkey,
            mut buffer_seconds,
            mut quality,
            mut framerate,
            mut resolution,
            mut capture_audio,
            mut audio_device,
            audio_devices,
            output_dir,
        ) = {
            let rec = self.recorder.lock().unwrap();
            (
                rec.settings.hotkey,
                rec.settings.buffer_seconds,
                rec.settings.quality,
                rec.settings.framerate,
                rec.settings.resolution,
                rec.settings.capture_audio,
                rec.settings.audio_device.clone(),
                rec.audio_devices.clone(),
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
                            egui::Button::new("📂 Browse")
                                .fill(egui::Color32::from_rgb(50, 55, 65))
                                .corner_radius(4.0),
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
                        egui::RichText::new("📂  Open Clips Folder")
                            .color(egui::Color32::from_rgb(160, 180, 210)),
                    )
                    .fill(egui::Color32::from_rgb(40, 45, 55))
                    .corner_radius(4.0),
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
            rec.settings.hotkey = hotkey;
            rec.settings.buffer_seconds = buffer_seconds;
            rec.settings.quality = quality;
            rec.settings.framerate = framerate;
            rec.settings.resolution = resolution;
            rec.settings.capture_audio = capture_audio;
            rec.settings.audio_device = audio_device;
        }

        // Signal hotkey change if needed
        if hotkey != original_hotkey {
            self.pending_hotkey = Some(hotkey);
        }
    }
}

impl eframe::App for LiteClipApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Request repaint for timer and status fade
        ctx.request_repaint_after(std::time::Duration::from_millis(100));

        // Decrease status timer
        if self.status_timer > 0.0 {
            self.status_timer -= 0.1;
        }

        // Dark theme
        let mut visuals = egui::Visuals::dark();
        visuals.override_text_color = Some(egui::Color32::from_rgb(210, 215, 225));
        visuals.window_fill = egui::Color32::from_rgb(22, 24, 30);
        visuals.panel_fill = egui::Color32::from_rgb(22, 24, 30);
        visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(35, 38, 48);
        visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(45, 50, 62);
        visuals.widgets.active.bg_fill = egui::Color32::from_rgb(55, 60, 75);
        ctx.set_visuals(visuals);

        egui::CentralPanel::default()
            .frame(egui::Frame::central_panel(&ctx.style()).inner_margin(12.0))
            .show(ctx, |ui| {
                // --- Header ---
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("⚡")
                            .size(20.0)
                            .color(egui::Color32::from_rgb(80, 160, 255)),
                    );
                    ui.label(
                        egui::RichText::new("LiteClip")
                            .size(18.0)
                            .color(egui::Color32::from_rgb(200, 210, 235))
                            .strong(),
                    );
                    ui.label(
                        egui::RichText::new("v0.2")
                            .small()
                            .color(egui::Color32::from_rgb(80, 90, 110)),
                    );
                });

                ui.add_space(4.0);

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
        ui.label(egui::RichText::new(label).color(egui::Color32::from_rgb(180, 190, 210)));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), add_widget);
    });
}

/// Section header divider.
fn section_header(ui: &mut egui::Ui, title: &str) {
    ui.label(
        egui::RichText::new(format!("─── {} ───", title))
            .small()
            .color(egui::Color32::from_rgb(100, 110, 130)),
    );
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
