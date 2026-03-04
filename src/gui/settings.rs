use eframe::egui;

use tokio::sync::mpsc::Sender;

use crate::config::{config_mod::types::*, Config};
use crate::platform::AppEvent;



pub fn show_settings_gui(event_tx: Sender<AppEvent>) {
    crate::gui::manager::send_gui_message(crate::gui::manager::GuiMessage::ShowSettings(event_tx));
}

pub struct SettingsApp {
    pub config: Config,
    pub event_tx: Sender<AppEvent>,
    pub save_status: Option<String>,
}

impl SettingsApp {
    pub fn update(&mut self, ctx: &egui::Context, is_open: &mut bool) {
        self.render(ctx, is_open);
    }

    fn render(&mut self, ctx: &egui::Context, is_open: &mut bool) {
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
                                self.config.general.save_directory =
                                    folder.to_string_lossy().to_string();
                            }
                        }
                    });

                    ui.add(
                        egui::Slider::new(&mut self.config.general.replay_duration_secs, 5..=300)
                            .text("Replay Duration (s)"),
                    );
                    ui.checkbox(
                        &mut self.config.general.auto_start_with_windows,
                        "Auto Start with Windows",
                    );
                    ui.checkbox(&mut self.config.general.start_minimised, "Start Minimised");
                    ui.checkbox(
                        &mut self.config.general.notifications,
                        "Enable Notifications",
                    );
                    ui.checkbox(
                        &mut self.config.general.auto_detect_game,
                        "Auto Detect Game",
                    );
                });

                // Video Settings
                ui.collapsing("Video", |ui| {
                    ui.checkbox(
                        &mut self.config.video.use_native_resolution,
                        "Use Native Resolution",
                    );
                    if !self.config.video.use_native_resolution {
                        egui::ComboBox::from_label("Resolution")
                            .selected_text(format!("{:?}", self.config.video.resolution))
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut self.config.video.resolution,
                                    Resolution::Native,
                                    "Native",
                                );
                                ui.selectable_value(
                                    &mut self.config.video.resolution,
                                    Resolution::P1080,
                                    "1080p",
                                );
                                ui.selectable_value(
                                    &mut self.config.video.resolution,
                                    Resolution::P720,
                                    "720p",
                                );
                                ui.selectable_value(
                                    &mut self.config.video.resolution,
                                    Resolution::P480,
                                    "480p",
                                );
                            });
                    }

                    ui.add(
                        egui::Slider::new(&mut self.config.video.framerate, 10..=144)
                            .text("Framerate"),
                    );
                    ui.add(
                        egui::Slider::new(&mut self.config.video.bitrate_mbps, 1..=150)
                            .text("Bitrate (Mbps)"),
                    );

                    egui::ComboBox::from_label("Codec")
                        .selected_text(format!("{:?}", self.config.video.codec))
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.config.video.codec, Codec::H264, "H.264");
                            ui.selectable_value(
                                &mut self.config.video.codec,
                                Codec::H265,
                                "H.265 / HEVC",
                            );
                            ui.selectable_value(&mut self.config.video.codec, Codec::Av1, "AV1");
                        });

                    egui::ComboBox::from_label("Encoder")
                        .selected_text(format!("{:?}", self.config.video.encoder))
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut self.config.video.encoder,
                                EncoderType::Auto,
                                "Auto",
                            );
                            ui.selectable_value(
                                &mut self.config.video.encoder,
                                EncoderType::Software,
                                "Software (CPU)",
                            );
                            ui.selectable_value(
                                &mut self.config.video.encoder,
                                EncoderType::Nvenc,
                                "NVENC (NVIDIA)",
                            );
                            ui.selectable_value(
                                &mut self.config.video.encoder,
                                EncoderType::Amf,
                                "AMF (AMD)",
                            );
                            ui.selectable_value(
                                &mut self.config.video.encoder,
                                EncoderType::Qsv,
                                "QSV (Intel)",
                            );
                        });

                    egui::ComboBox::from_label("Quality Preset")
                        .selected_text(format!("{:?}", self.config.video.quality_preset))
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut self.config.video.quality_preset,
                                QualityPreset::Performance,
                                "Performance",
                            );
                            ui.selectable_value(
                                &mut self.config.video.quality_preset,
                                QualityPreset::Balanced,
                                "Balanced",
                            );
                            ui.selectable_value(
                                &mut self.config.video.quality_preset,
                                QualityPreset::Quality,
                                "Quality",
                            );
                        });

                    egui::ComboBox::from_label("Rate Control")
                        .selected_text(format!("{:?}", self.config.video.rate_control))
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut self.config.video.rate_control,
                                RateControl::Cbr,
                                "CBR",
                            );
                            ui.selectable_value(
                                &mut self.config.video.rate_control,
                                RateControl::Vbr,
                                "VBR",
                            );
                            ui.selectable_value(
                                &mut self.config.video.rate_control,
                                RateControl::Cq,
                                "CQ",
                            );
                        });

                    if self.config.video.rate_control == RateControl::Cq {
                        let mut cq_val = self.config.video.quality_value.unwrap_or(23);
                        ui.add(egui::Slider::new(&mut cq_val, 1..=51).text("CQ Level"));
                        self.config.video.quality_value = Some(cq_val);
                    }
                });

                // Audio Settings
                ui.collapsing("Audio", |ui| {
                    ui.checkbox(
                        &mut self.config.audio.capture_system,
                        "Capture System Audio",
                    );
                    ui.add(
                        egui::Slider::new(&mut self.config.audio.system_volume, 0..=200)
                            .text("System Volume %"),
                    );

                    ui.checkbox(&mut self.config.audio.capture_mic, "Capture Microphone");
                    ui.text_edit_singleline(&mut self.config.audio.mic_device);
                    ui.add(
                        egui::Slider::new(&mut self.config.audio.mic_volume, 0..=200)
                            .text("Mic Volume %"),
                    );
                    ui.checkbox(
                        &mut self.config.audio.mic_noise_reduction,
                        "Mic Noise Reduction (AI)",
                    );
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
                    ui.add(
                        egui::Slider::new(&mut self.config.advanced.memory_limit_mb, 128..=4096)
                            .text("Memory Limit (MB)"),
                    );
                    ui.add(
                        egui::Slider::new(&mut self.config.advanced.gpu_index, 0..=4)
                            .text("GPU Index"),
                    );
                    ui.add(
                        egui::Slider::new(&mut self.config.advanced.keyframe_interval_secs, 1..=10)
                            .text("Keyframe Interval (s)"),
                    );
                    ui.checkbox(
                        &mut self.config.advanced.use_cpu_readback,
                        "Use CPU Readback for HW Encoding",
                    );
                    ui.checkbox(
                        &mut self.config.advanced.overlay_enabled,
                        "Enable In-Game Overlay",
                    );
                    if self.config.advanced.overlay_enabled {
                        egui::ComboBox::from_label("Overlay Position")
                            .selected_text(format!("{:?}", self.config.advanced.overlay_position))
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut self.config.advanced.overlay_position,
                                    OverlayPosition::TopLeft,
                                    "Top Left",
                                );
                                ui.selectable_value(
                                    &mut self.config.advanced.overlay_position,
                                    OverlayPosition::TopRight,
                                    "Top Right",
                                );
                                ui.selectable_value(
                                    &mut self.config.advanced.overlay_position,
                                    OverlayPosition::BottomLeft,
                                    "Bottom Left",
                                );
                                ui.selectable_value(
                                    &mut self.config.advanced.overlay_position,
                                    OverlayPosition::BottomRight,
                                    "Bottom Right",
                                );
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
                        Ok(_) => match self.event_tx.try_send(AppEvent::Restart) {
                            Ok(_) => {
                                 *is_open = false;
                                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            }
                            Err(e) => {
                                self.save_status =
                                    Some(format!("Saved, but restart signal failed: {}", e));
                            }
                        },
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

impl eframe::App for SettingsApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let mut _dummy = true;
        self.render(ctx, &mut _dummy);
    }
}
