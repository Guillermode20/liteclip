use eframe::egui;
use log::{error, info, warn};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::recorder::{Recorder, RecorderState};
use crate::settings::{
    self, EncoderTuning, Framerate, HotkeyPreset, Quality, RateControl, Resolution, VideoEncoder,
};

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
            let result = Recorder::save_clip_auto_detached(&recorder);
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
        // Color constants
        const ACCENT: egui::Color32 = egui::Color32::from_rgb(108, 99, 255);
        const RED_INDICATOR: egui::Color32 = egui::Color32::from_rgb(255, 64, 96);
        const TEXT_SECONDARY: egui::Color32 = egui::Color32::from_rgb(136, 144, 160);
        const TEXT_MUTED: egui::Color32 = egui::Color32::from_rgb(80, 88, 104);
        const BG_ELEVATED: egui::Color32 = egui::Color32::from_rgb(24, 24, 28);
        const BORDER: egui::Color32 = egui::Color32::from_rgb(28, 28, 34);

        self.refresh_main_snapshot();
        let state = self.cached_state;
        let ffmpeg_found = self.cached_ffmpeg_found;
        let last_saved = self.cached_last_saved_path.clone();
        let hotkey_label = self.cached_hotkey_label.clone();

        // --- FFmpeg warning ---
        if !ffmpeg_found {
            ui.vertical_centered(|ui| {
                ui.add_space(16.0);
                ui.label(
                    egui::RichText::new("⚠  FFmpeg Not Found")
                        .size(15.0)
                        .color(RED_INDICATOR)
                        .strong(),
                );
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(
                        "Install FFmpeg and add it to PATH,\nthen restart LiteClip.",
                    )
                    .color(TEXT_SECONDARY),
                );
            });
            return;
        }

        // --- Status area ---
        ui.add_space(4.0);
        ui.vertical_centered(|ui| {
            match state {
                RecorderState::Idle => {
                    ui.label(
                        egui::RichText::new("STANDBY")
                            .size(14.0)
                            .color(TEXT_MUTED)
                            .strong(),
                    );
                }
                RecorderState::Recording => {
                    // Pulsing dot with glow
                    let t = ui.input(|i| i.time);
                    let pulse = ((t * 2.5).sin() * 0.5 + 0.5) as f32;
                    let r = (64.0 + pulse * 191.0) as u8;
                    let g = (16.0 + pulse * 48.0) as u8;
                    let b = (32.0 + pulse * 64.0) as u8;
                    ui.horizontal(|ui| {
                        ui.add_space(ui.available_width() / 2.0 - 30.0);
                        ui.label(
                            egui::RichText::new("●")
                                .size(14.0)
                                .color(egui::Color32::from_rgb(r, g, b)),
                        );
                        ui.label(
                            egui::RichText::new("REC")
                                .size(14.0)
                                .color(RED_INDICATOR)
                                .strong(),
                        );
                    });
                }
                RecorderState::Saving => {
                    ui.label(
                        egui::RichText::new("SAVING...")
                            .size(14.0)
                            .color(ACCENT)
                            .strong(),
                    );
                }
            }
        });

        ui.add_space(8.0);

        // --- Control buttons ---
        ui.horizontal(|ui| {
            let avail = ui.available_width();
            let btn_w = (avail - 8.0) / 2.0;
            let btn_size = egui::vec2(btn_w, 34.0);

            match state {
                RecorderState::Idle => {
                    let start_btn = ui.add_enabled(
                        !self.start_in_progress,
                        egui::Button::new(
                            egui::RichText::new(if self.start_in_progress {
                                "INITIALIZING..."
                            } else {
                                "▶ START BUFFER"
                            })
                            .size(13.0)
                            .color(egui::Color32::WHITE),
                        )
                        .fill(BG_ELEVATED)
                        .stroke(egui::Stroke::new(1.0, BORDER))
                        .corner_radius(6.0)
                        .min_size(egui::vec2(avail, 34.0)),
                    );
                    if start_btn.clicked() {
                        self.trigger_start();
                    }
                }
                RecorderState::Recording => {
                    // Save clip button — accent filled
                    if ui
                        .add_sized(
                            btn_size,
                            egui::Button::new(
                                egui::RichText::new(format!("⬇ SAVE [{}]", hotkey_label))
                                    .size(12.0)
                                    .color(egui::Color32::WHITE)
                                    .strong(),
                            )
                            .fill(ACCENT)
                            .corner_radius(6.0),
                        )
                        .clicked()
                    {
                        self.trigger_save();
                    }

                    // Stop button — outlined red
                    if ui
                        .add_sized(
                            btn_size,
                            egui::Button::new(
                                egui::RichText::new("■ STOP")
                                    .size(12.0)
                                    .color(RED_INDICATOR),
                            )
                            .fill(egui::Color32::from_rgb(20, 12, 16))
                            .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(80, 30, 40)))
                            .corner_radius(6.0),
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
                        egui::Button::new(
                            egui::RichText::new("Saving...").size(13.0).color(ACCENT),
                        )
                        .frame(false)
                        .min_size(egui::vec2(avail, 34.0)),
                    );
                }
            }
        });

        // --- Status toast ---
        if self.status_timer > 0.0 {
            ui.add_space(6.0);
            let alpha = (self.status_timer.min(1.0) * 255.0) as u8;
            ui.label(
                egui::RichText::new(&self.status_message)
                    .size(11.0)
                    .color(egui::Color32::from_rgba_premultiplied(108, 99, 255, alpha)),
            );
        }

        // --- Footer ---
        ui.add_space(4.0);
        // Thin separator
        let rect = ui.available_rect_before_wrap();
        ui.painter().line_segment(
            [
                egui::pos2(rect.left(), rect.top()),
                egui::pos2(rect.right(), rect.top()),
            ],
            egui::Stroke::new(1.0, BORDER),
        );
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if let Some(ref path) = last_saved {
                let name = path.file_name().unwrap_or_default().to_str().unwrap_or("");
                ui.label(
                    egui::RichText::new(format!("Last: {}", name))
                        .size(10.0)
                        .color(TEXT_MUTED),
                );
            } else {
                ui.label(
                    egui::RichText::new("No clips yet")
                        .size(10.0)
                        .color(TEXT_MUTED),
                );
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(
                        egui::Button::new(
                            egui::RichText::new("⚙ Settings")
                                .size(10.0)
                                .color(TEXT_SECONDARY),
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
        const TEXT_SECONDARY: egui::Color32 = egui::Color32::from_rgb(136, 144, 160);
        const TEXT_PRIMARY: egui::Color32 = egui::Color32::from_rgb(232, 236, 240);
        const BG_ELEVATED: egui::Color32 = egui::Color32::from_rgb(24, 24, 28);
        const BORDER: egui::Color32 = egui::Color32::from_rgb(28, 28, 34);

        ui.horizontal(|ui| {
            if ui
                .add(
                    egui::Button::new(
                        egui::RichText::new("← Back")
                            .size(12.0)
                            .color(TEXT_SECONDARY),
                    )
                    .frame(false),
                )
                .clicked()
            {
                self.panel = Panel::Main;
            }
            ui.label(
                egui::RichText::new("Settings")
                    .size(16.0)
                    .color(TEXT_PRIMARY)
                    .strong(),
            );
        });

        // Thin separator
        ui.add_space(4.0);
        let rect = ui.available_rect_before_wrap();
        ui.painter().line_segment(
            [
                egui::pos2(rect.left(), rect.top()),
                egui::pos2(rect.right(), rect.top()),
            ],
            egui::Stroke::new(1.0, BORDER),
        );
        ui.add_space(4.0);

        // Snapshot current settings to avoid holding lock across closures
        let (
            mut hotkey,
            mut buffer_seconds,
            mut quality,
            mut video_encoder,
            mut framerate,
            mut resolution,
            mut advanced_video_controls,
            mut rate_control,
            mut encoder_tuning,
            mut video_bitrate_kbps,
            mut video_max_bitrate_kbps,
            mut video_bufsize_kbps,
            mut video_crf,
            mut keyframe_interval_sec,
            mut custom_resolution_enabled,
            mut custom_resolution_width,
            mut custom_resolution_height,
            mut audio_bitrate_kbps,
            mut capture_audio,
            mut audio_device,
            mut launch_on_startup,
            mut minimize_to_tray,
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
                rec.settings.advanced_video_controls,
                rec.settings.rate_control,
                rec.settings.encoder_tuning,
                rec.settings.video_bitrate_kbps,
                rec.settings.video_max_bitrate_kbps,
                rec.settings.video_bufsize_kbps,
                rec.settings.video_crf,
                rec.settings.keyframe_interval_sec,
                rec.settings.custom_resolution_enabled,
                rec.settings.custom_resolution_width,
                rec.settings.custom_resolution_height,
                rec.settings.audio_bitrate_kbps,
                rec.settings.capture_audio,
                rec.settings.audio_device.clone(),
                rec.settings.launch_on_startup,
                rec.settings.minimize_to_tray,
                rec.audio_devices.clone(),
                rec.video_encoders.clone(),
                rec.settings.output_dir.clone(),
            )
        };

        let original_launch_on_startup = launch_on_startup;

        let original_hotkey = hotkey;

        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.spacing_mut().item_spacing = egui::vec2(8.0, 6.0);

            // --- GENERAL ---
            section_header(ui, "GENERAL");

            setting_row(ui, "Launch on Startup", |ui| {
                ui.checkbox(&mut launch_on_startup, "");
            });

            setting_row(ui, "Minimize to Tray", |ui| {
                ui.checkbox(&mut minimize_to_tray, "");
            });

            ui.add_space(2.0);

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

            setting_row(ui, "Advanced Video", |ui| {
                ui.checkbox(&mut advanced_video_controls, "");
            });

            if advanced_video_controls {
                setting_row(ui, "Rate Control", |ui| {
                    egui::ComboBox::from_id_salt("rate_control_sel")
                        .selected_text(rate_control.label())
                        .width(150.0)
                        .show_ui(ui, |ui| {
                            for mode in RateControl::all() {
                                ui.selectable_value(&mut rate_control, *mode, mode.label());
                            }
                        });
                });

                setting_row(ui, "Encoder Tuning", |ui| {
                    egui::ComboBox::from_id_salt("encoder_tuning_sel")
                        .selected_text(encoder_tuning.label())
                        .width(150.0)
                        .show_ui(ui, |ui| {
                            for tune in EncoderTuning::all() {
                                ui.selectable_value(&mut encoder_tuning, *tune, tune.label());
                            }
                        });
                });

                if rate_control != RateControl::Preset {
                    setting_row(ui, "Video Bitrate (kbps)", |ui| {
                        ui.add(
                            egui::DragValue::new(&mut video_bitrate_kbps)
                                .range(1000..=150000)
                                .speed(100.0),
                        );
                    });
                }

                if rate_control == RateControl::Vbr {
                    setting_row(ui, "Max Bitrate (kbps)", |ui| {
                        ui.add(
                            egui::DragValue::new(&mut video_max_bitrate_kbps)
                                .range(1000..=200000)
                                .speed(100.0),
                        );
                    });
                }

                if matches!(rate_control, RateControl::Cbr | RateControl::Vbr) {
                    setting_row(ui, "Buffer Size (kbps)", |ui| {
                        ui.add(
                            egui::DragValue::new(&mut video_bufsize_kbps)
                                .range(1000..=400000)
                                .speed(100.0),
                        );
                    });
                }

                if rate_control == RateControl::Crf {
                    setting_row(ui, "CRF", |ui| {
                        ui.add(
                            egui::DragValue::new(&mut video_crf)
                                .range(0..=51)
                                .speed(1.0),
                        );
                    });
                }

                setting_row(ui, "Keyframe Sec", |ui| {
                    ui.add(
                        egui::DragValue::new(&mut keyframe_interval_sec)
                            .range(1..=10)
                            .speed(1.0),
                    );
                });

                setting_row(ui, "Custom Resolution", |ui| {
                    ui.checkbox(&mut custom_resolution_enabled, "");
                });

                if custom_resolution_enabled {
                    setting_row(ui, "Custom Width", |ui| {
                        ui.add(
                            egui::DragValue::new(&mut custom_resolution_width)
                                .range(320..=7680)
                                .speed(2.0),
                        );
                    });

                    setting_row(ui, "Custom Height", |ui| {
                        ui.add(
                            egui::DragValue::new(&mut custom_resolution_height)
                                .range(240..=4320)
                                .speed(2.0),
                        );
                    });
                }
            }

            ui.add_space(2.0);
            section_header(ui, "AUDIO");

            // --- Audio toggle ---
            setting_row(ui, "Capture Audio", |ui| {
                ui.checkbox(&mut capture_audio, "");
            });

            // --- Audio device selector ---
            if capture_audio {
                setting_row(ui, "Audio Bitrate", |ui| {
                    ui.add(
                        egui::DragValue::new(&mut audio_bitrate_kbps)
                            .range(64..=512)
                            .speed(8.0),
                    );
                });

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
                ui.label(egui::RichText::new("Save Folder").color(TEXT_SECONDARY));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new("Browse").size(11.0).color(TEXT_PRIMARY),
                            )
                            .fill(BG_ELEVATED)
                            .stroke(egui::Stroke::new(1.0, BORDER))
                            .corner_radius(6.0),
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
                    .color(egui::Color32::from_rgb(80, 88, 104)),
            );

            // --- Open output folder ---
            ui.add_space(6.0);
            if ui
                .add(
                    egui::Button::new(
                        egui::RichText::new("Open Clips Folder")
                            .size(11.0)
                            .color(TEXT_PRIMARY),
                    )
                    .fill(BG_ELEVATED)
                    .stroke(egui::Stroke::new(1.0, BORDER))
                    .corner_radius(6.0),
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
            video_max_bitrate_kbps = video_max_bitrate_kbps.max(video_bitrate_kbps);
            video_bufsize_kbps = video_bufsize_kbps.max(video_bitrate_kbps);

            let mut buffer_change_restart_result: Option<Result<(), String>> = None;
            let mut rec = self.recorder.lock().unwrap();
            let buffer_seconds_changed = rec.settings.buffer_seconds != buffer_seconds;

            // Persist recorder settings that are applied immediately in-process.
            // Hotkey is intentionally excluded and only persisted after successful
            // registration in main.rs.
            let changed = rec.settings.buffer_seconds != buffer_seconds
                || rec.settings.quality != quality
                || rec.settings.video_encoder != video_encoder
                || rec.settings.framerate != framerate
                || rec.settings.resolution != resolution
                || rec.settings.advanced_video_controls != advanced_video_controls
                || rec.settings.rate_control != rate_control
                || rec.settings.encoder_tuning != encoder_tuning
                || rec.settings.video_bitrate_kbps != video_bitrate_kbps
                || rec.settings.video_max_bitrate_kbps != video_max_bitrate_kbps
                || rec.settings.video_bufsize_kbps != video_bufsize_kbps
                || rec.settings.video_crf != video_crf
                || rec.settings.keyframe_interval_sec != keyframe_interval_sec
                || rec.settings.custom_resolution_enabled != custom_resolution_enabled
                || rec.settings.custom_resolution_width != custom_resolution_width
                || rec.settings.custom_resolution_height != custom_resolution_height
                || rec.settings.audio_bitrate_kbps != audio_bitrate_kbps
                || rec.settings.capture_audio != capture_audio
                || rec.settings.audio_device != audio_device
                || rec.settings.launch_on_startup != launch_on_startup
                || rec.settings.minimize_to_tray != minimize_to_tray;

            rec.settings.buffer_seconds = buffer_seconds;
            rec.settings.quality = quality;
            rec.settings.video_encoder = video_encoder;
            rec.settings.framerate = framerate;
            rec.settings.resolution = resolution;
            rec.settings.advanced_video_controls = advanced_video_controls;
            rec.settings.rate_control = rate_control;
            rec.settings.encoder_tuning = encoder_tuning;
            rec.settings.video_bitrate_kbps = video_bitrate_kbps;
            rec.settings.video_max_bitrate_kbps = video_max_bitrate_kbps;
            rec.settings.video_bufsize_kbps = video_bufsize_kbps;
            rec.settings.video_crf = video_crf;
            rec.settings.keyframe_interval_sec = keyframe_interval_sec;
            rec.settings.custom_resolution_enabled = custom_resolution_enabled;
            rec.settings.custom_resolution_width = custom_resolution_width;
            rec.settings.custom_resolution_height = custom_resolution_height;
            rec.settings.audio_bitrate_kbps = audio_bitrate_kbps;
            rec.settings.capture_audio = capture_audio;
            rec.settings.audio_device = audio_device;
            rec.settings.launch_on_startup = launch_on_startup;
            rec.settings.minimize_to_tray = minimize_to_tray;

            if changed {
                rec.settings.save();
                info!("Settings saved to disk");
            }

            if buffer_seconds_changed && rec.state == RecorderState::Recording {
                info!(
                    "Buffer length changed while recording ({}s) — restarting recorder to apply",
                    buffer_seconds
                );
                rec.stop();
                buffer_change_restart_result = Some(rec.start());
            }

            if let Some(result) = buffer_change_restart_result {
                match result {
                    Ok(()) => {
                        self.status_message =
                            format!("Buffer length updated to {}.", format_buffer_length(buffer_seconds));
                        self.status_timer = 3.0;
                    }
                    Err(err) => {
                        self.status_message =
                            format!("Buffer updated, but restart failed: {}", err);
                        self.status_timer = 5.0;
                    }
                }
            }
        }

        // Apply startup registry change if toggled
        if launch_on_startup != original_launch_on_startup {
            settings::set_launch_on_startup(launch_on_startup);
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

        // ── Color palette constants ──
        const BG_PRIMARY: egui::Color32 = egui::Color32::from_rgb(10, 10, 12);
        const BG_ELEVATED: egui::Color32 = egui::Color32::from_rgb(24, 24, 28);
        const ACCENT: egui::Color32 = egui::Color32::from_rgb(108, 99, 255);
        const ACCENT_HOVER: egui::Color32 = egui::Color32::from_rgb(123, 115, 255);
        const ACCENT_MUTED: egui::Color32 = egui::Color32::from_rgb(42, 38, 64);
        const TEXT_PRIMARY: egui::Color32 = egui::Color32::from_rgb(232, 236, 240);
        const TEXT_SECONDARY: egui::Color32 = egui::Color32::from_rgb(136, 144, 160);
        const BORDER: egui::Color32 = egui::Color32::from_rgb(28, 28, 34);

        if !self.visuals_initialized {
            let mut visuals = egui::Visuals::dark();
            visuals.override_text_color = Some(TEXT_PRIMARY);
            visuals.window_fill = BG_PRIMARY;
            visuals.panel_fill = BG_PRIMARY;

            visuals.widgets.noninteractive.bg_fill = BG_PRIMARY;
            visuals.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, BORDER);
            visuals.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, TEXT_SECONDARY);

            visuals.widgets.inactive.bg_fill = BG_ELEVATED;
            visuals.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, TEXT_PRIMARY);
            visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, BORDER);

            visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(30, 30, 38);
            visuals.widgets.hovered.fg_stroke = egui::Stroke::new(1.0, egui::Color32::WHITE);
            visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, ACCENT);

            visuals.widgets.active.bg_fill = ACCENT_MUTED;
            visuals.widgets.active.fg_stroke = egui::Stroke::new(1.0, egui::Color32::WHITE);
            visuals.widgets.active.bg_stroke = egui::Stroke::new(1.0, ACCENT_HOVER);

            visuals.selection.bg_fill = ACCENT_MUTED;
            visuals.selection.stroke = egui::Stroke::new(1.0, ACCENT);

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
            .frame(egui::Frame::central_panel(&ctx.style()).inner_margin(16.0))
            .show(ctx, |ui| {
                // --- Header ---
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("●").size(10.0).color(ACCENT));
                    ui.add_space(2.0);
                    ui.label(
                        egui::RichText::new("LITECLIP")
                            .size(15.0)
                            .color(TEXT_PRIMARY)
                            .strong(),
                    );
                    ui.label(
                        egui::RichText::new("REPLAY")
                            .size(15.0)
                            .color(TEXT_SECONDARY),
                    );
                });

                // Thin accent separator line
                ui.add_space(6.0);
                let rect = ui.available_rect_before_wrap();
                let line_y = rect.top();
                ui.painter().line_segment(
                    [
                        egui::pos2(rect.left(), line_y),
                        egui::pos2(rect.right(), line_y),
                    ],
                    egui::Stroke::new(1.0, BORDER),
                );
                ui.add_space(10.0);

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
        ui.label(
            egui::RichText::new(label)
                .size(11.0)
                .color(egui::Color32::from_rgb(136, 144, 160)),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), add_widget);
    });
}

/// Section header with accent indicator bar.
fn section_header(ui: &mut egui::Ui, title: &str) {
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        // Accent bar indicator
        let (rect, _) = ui.allocate_exact_size(egui::vec2(3.0, 14.0), egui::Sense::hover());
        ui.painter().rect_filled(
            rect,
            egui::CornerRadius::same(2),
            egui::Color32::from_rgb(108, 99, 255),
        );
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(title)
                .size(11.0)
                .color(egui::Color32::from_rgb(232, 236, 240))
                .strong(),
        );
    });
    ui.add_space(2.0);
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
    if max_len == 0 {
        return String::new();
    }

    let mut chars = s.chars();
    let visible: String = chars.by_ref().take(max_len).collect();
    if chars.next().is_none() {
        return visible;
    }

    if max_len == 1 {
        "…".to_string()
    } else {
        let prefix: String = s.chars().take(max_len - 1).collect();
        format!("{}…", prefix)
    }
}
