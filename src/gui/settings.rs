use eframe::egui;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::mpsc::Sender;

use crate::config::{Config, config_mod::types::*};
use crate::platform::AppEvent;

static IS_GUI_OPEN: AtomicBool = AtomicBool::new(false);

pub fn run_settings_gui(event_tx: Sender<AppEvent>) {
    if IS_GUI_OPEN.swap(true, Ordering::SeqCst) {
        return; // Already open
    }

    let config = match Config::load_sync() {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("Failed to load config for GUI: {}", e);
            Config::default()
        }
    };

    let app = SettingsApp {
        config,
        event_tx,
        save_status: None,
    };

    std::thread::spawn(move || {
        let options = eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_inner_size([600.0, 700.0])
                .with_title("LiteClip Replay Settings"),
            ..Default::default()
        };

        let _ = eframe::run_native(
            "liteclip_replay_settings",
            options,
            Box::new(|_cc| Ok(Box::new(app))),
        );
        IS_GUI_OPEN.store(false, Ordering::SeqCst);
    });
}

struct SettingsApp {
    config: Config,
    event_tx: Sender<AppEvent>,
    save_status: Option<String>,
}

impl eframe::App for SettingsApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("LiteClip Replay Settings");
            ui.separator();

            egui::ScrollArea::vertical().show(ui, |ui| {
                // General Settings
                ui.collapsing("General", |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Save Directory:");
                        ui.text_edit_singleline(&mut self.config.general.save_directory);
                        if ui.button("Browse...").clicked() {
                            if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                                self.config.general.save_directory = folder.to_string_lossy().to_string();
                            }
                        }
                    });

                    ui.add(egui::Slider::new(&mut self.config.general.replay_duration_secs, 5..=300).text("Replay Duration (s)"));
                    ui.checkbox(&mut self.config.general.auto_start_with_windows, "Auto Start with Windows");
                    ui.checkbox(&mut self.config.general.start_minimised, "Start Minimised");
                    ui.checkbox(&mut self.config.general.notifications, "Enable Notifications");
                    ui.checkbox(&mut self.config.general.auto_detect_game, "Auto Detect Game");
                });

                // Video Settings
                ui.collapsing("Video", |ui| {
                    ui.checkbox(&mut self.config.video.use_native_resolution, "Use Native Resolution");
                    if !self.config.video.use_native_resolution {
                        egui::ComboBox::from_label("Resolution")
                            .selected_text(format!("{:?}", self.config.video.resolution))
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut self.config.video.resolution, Resolution::Native, "Native");
                                ui.selectable_value(&mut self.config.video.resolution, Resolution::P1080, "1080p");
                                ui.selectable_value(&mut self.config.video.resolution, Resolution::P720, "720p");
                                ui.selectable_value(&mut self.config.video.resolution, Resolution::P480, "480p");
                            });
                    }

                    ui.add(egui::Slider::new(&mut self.config.video.framerate, 10..=144).text("Framerate"));
                    ui.add(egui::Slider::new(&mut self.config.video.bitrate_mbps, 1..=150).text("Bitrate (Mbps)"));
                    
                    egui::ComboBox::from_label("Codec")
                        .selected_text(format!("{:?}", self.config.video.codec))
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.config.video.codec, Codec::H264, "H.264");
                            ui.selectable_value(&mut self.config.video.codec, Codec::H265, "H.265 / HEVC");
                            ui.selectable_value(&mut self.config.video.codec, Codec::Av1, "AV1");
                        });

                    egui::ComboBox::from_label("Encoder")
                        .selected_text(format!("{:?}", self.config.video.encoder))
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.config.video.encoder, EncoderType::Auto, "Auto");
                            ui.selectable_value(&mut self.config.video.encoder, EncoderType::Software, "Software (CPU)");
                            ui.selectable_value(&mut self.config.video.encoder, EncoderType::Nvenc, "NVENC (NVIDIA)");
                            ui.selectable_value(&mut self.config.video.encoder, EncoderType::Amf, "AMF (AMD)");
                            ui.selectable_value(&mut self.config.video.encoder, EncoderType::Qsv, "QSV (Intel)");
                        });
                        
                    egui::ComboBox::from_label("Quality Preset")
                        .selected_text(format!("{:?}", self.config.video.quality_preset))
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.config.video.quality_preset, QualityPreset::Performance, "Performance");
                            ui.selectable_value(&mut self.config.video.quality_preset, QualityPreset::Balanced, "Balanced");
                            ui.selectable_value(&mut self.config.video.quality_preset, QualityPreset::Quality, "Quality");
                        });

                    egui::ComboBox::from_label("Rate Control")
                        .selected_text(format!("{:?}", self.config.video.rate_control))
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.config.video.rate_control, RateControl::Cbr, "CBR");
                            ui.selectable_value(&mut self.config.video.rate_control, RateControl::Vbr, "VBR");
                            ui.selectable_value(&mut self.config.video.rate_control, RateControl::Cq, "CQ");
                        });

                    if self.config.video.rate_control == RateControl::Cq {
                        let mut cq_val = self.config.video.quality_value.unwrap_or(23);
                        ui.add(egui::Slider::new(&mut cq_val, 1..=51).text("CQ Level"));
                        self.config.video.quality_value = Some(cq_val);
                    }
                });

                // Audio Settings
                ui.collapsing("Audio", |ui| {
                    ui.checkbox(&mut self.config.audio.capture_system, "Capture System Audio");
                    ui.add(egui::Slider::new(&mut self.config.audio.system_volume, 0..=200).text("System Volume %"));
                    
                    ui.checkbox(&mut self.config.audio.capture_mic, "Capture Microphone");
                    ui.text_edit_singleline(&mut self.config.audio.mic_device);
                    ui.add(egui::Slider::new(&mut self.config.audio.mic_volume, 0..=200).text("Mic Volume %"));
                    ui.checkbox(&mut self.config.audio.mic_noise_reduction, "Mic Noise Reduction (AI)");
                });

                // Hotkey Settings
                ui.collapsing("Hotkeys", |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Save Clip:");
                        ui.text_edit_singleline(&mut self.config.hotkeys.save_clip);
                    });
                    ui.horizontal(|ui| {
                        ui.label("Toggle Recording:");
                        ui.text_edit_singleline(&mut self.config.hotkeys.toggle_recording);
                    });
                    ui.horizontal(|ui| {
                        ui.label("Screenshot:");
                        ui.text_edit_singleline(&mut self.config.hotkeys.screenshot);
                    });
                    ui.horizontal(|ui| {
                        ui.label("Open Gallery:");
                        ui.text_edit_singleline(&mut self.config.hotkeys.open_gallery);
                    });
                });

                // Advanced Settings
                ui.collapsing("Advanced", |ui| {
                    ui.add(egui::Slider::new(&mut self.config.advanced.memory_limit_mb, 128..=4096).text("Memory Limit (MB)"));
                    ui.add(egui::Slider::new(&mut self.config.advanced.gpu_index, 0..=4).text("GPU Index"));
                    ui.add(egui::Slider::new(&mut self.config.advanced.keyframe_interval_secs, 1..=10).text("Keyframe Interval (s)"));
                    ui.checkbox(&mut self.config.advanced.use_cpu_readback, "Use CPU Readback for HW Encoding");
                    ui.checkbox(&mut self.config.advanced.overlay_enabled, "Enable In-Game Overlay");
                    if self.config.advanced.overlay_enabled {
                        egui::ComboBox::from_label("Overlay Position")
                            .selected_text(format!("{:?}", self.config.advanced.overlay_position))
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut self.config.advanced.overlay_position, OverlayPosition::TopLeft, "Top Left");
                                ui.selectable_value(&mut self.config.advanced.overlay_position, OverlayPosition::TopRight, "Top Right");
                                ui.selectable_value(&mut self.config.advanced.overlay_position, OverlayPosition::BottomLeft, "Bottom Left");
                                ui.selectable_value(&mut self.config.advanced.overlay_position, OverlayPosition::BottomRight, "Bottom Right");
                            });
                    }
                });
            });

            ui.add_space(10.0);
            ui.separator();
            ui.add_space(5.0);

            ui.horizontal(|ui| {
                if ui.button("Save").clicked() {
                    self.config.validate();
                    match self.config.save_sync() {
                        Ok(_) => self.save_status = Some("Saved successfully!".to_string()),
                        Err(e) => self.save_status = Some(format!("Error: {}", e)),
                    }
                }

                if ui.button("Save & Restart App").clicked() {
                    self.config.validate();
                    match self.config.save_sync() {
                        Ok(_) => {
                            let _ = self.event_tx.try_send(AppEvent::Restart);
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                        Err(e) => self.save_status = Some(format!("Error: {}", e)),
                    }
                }

                if let Some(status) = &self.save_status {
                    ui.label(status);
                }
            });
        });
    }
}
