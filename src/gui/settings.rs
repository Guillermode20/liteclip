use eframe::egui;
use std::sync::Arc;
use tokio::sync::mpsc::Sender;

use crate::config::{config_mod::types::*, Config};
use crate::platform::AppEvent;

/// Shows the settings GUI window.
///
/// Spawns a new egui window for configuring application settings.
/// Changes are sent back to the main application via the event channel.
///
/// # Arguments
///
/// * `event_tx` - Channel to send configuration update events.
pub fn show_settings_gui(event_tx: Sender<AppEvent>) {
    crate::gui::manager::send_gui_message(crate::gui::manager::GuiMessage::ShowSettings(event_tx));
}

#[derive(Default)]
struct HotkeyValidationErrors {
    save_clip: Option<String>,
    toggle_recording: Option<String>,
    screenshot: Option<String>,
    open_gallery: Option<String>,
}

fn validate_hotkey(hotkey: &str) -> Result<(), String> {
    if hotkey.trim().is_empty() {
        return Err("Hotkey cannot be empty".to_string());
    }
    crate::platform::msg_loop::parse_hotkey(hotkey)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

fn render_hotkey_field(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut String,
    error: &mut Option<String>,
) {
    ui.horizontal(|ui| {
        ui.label(label);
        let response = ui.text_edit_singleline(value);

        if response.changed() {
            *error = validate_hotkey(value).err();
        }

        if let Some(ref err) = error {
            ui.colored_label(egui::Color32::RED, "⚠").on_hover_text(err);
        }
    });
}

pub struct SettingsApp {
    pub config: Config,
    pub event_tx: Sender<AppEvent>,
    pub save_status: Option<String>,
    hotkey_errors: HotkeyValidationErrors,
}

impl SettingsApp {
    pub fn new(config: Config, event_tx: Sender<AppEvent>) -> Self {
        Self {
            config,
            event_tx,
            save_status: None,
            hotkey_errors: HotkeyValidationErrors::default(),
        }
    }

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

                    // Codec is fixed to HEVC for performance
                    ui.label("Codec: HEVC (H.265)");

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
                    egui::ComboBox::from_label("Mic Noise Suppression")
                        .selected_text(match self.config.audio.mic_noise_suppression_mode {
                            MicNoiseSuppressionMode::Rnnoise => "RNNoise (Quality)",
                            MicNoiseSuppressionMode::NoiseGate => "Noise Gate (Performance)",
                            MicNoiseSuppressionMode::None => "Off",
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut self.config.audio.mic_noise_suppression_mode,
                                MicNoiseSuppressionMode::Rnnoise,
                                "RNNoise (Quality)",
                            );
                            ui.selectable_value(
                                &mut self.config.audio.mic_noise_suppression_mode,
                                MicNoiseSuppressionMode::NoiseGate,
                                "Noise Gate (Performance)",
                            );
                            ui.selectable_value(
                                &mut self.config.audio.mic_noise_suppression_mode,
                                MicNoiseSuppressionMode::None,
                                "Off",
                            );
                        });

                    if self.config.audio.mic_noise_suppression_mode
                        == MicNoiseSuppressionMode::Rnnoise
                    {
                        ui.indent("noise_suppression_settings", |ui| {
                            egui::ComboBox::from_label("Sensitivity")
                                .selected_text(
                                    match self.config.audio.mic_ns_vad_gate_threshold_percent {
                                        0..=49 => "Low",
                                        50..=64 => "Medium",
                                        _ => "High",
                                    },
                                )
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(
                                        &mut self.config.audio.mic_ns_vad_gate_threshold_percent,
                                        40,
                                        "Low",
                                    );
                                    ui.selectable_value(
                                        &mut self.config.audio.mic_ns_vad_gate_threshold_percent,
                                        55,
                                        "Medium",
                                    );
                                    ui.selectable_value(
                                        &mut self.config.audio.mic_ns_vad_gate_threshold_percent,
                                        70,
                                        "High",
                                    );
                                });
                            ui.label(
                                egui::RichText::new(
                                    "Low = whisper; High = aggressive suppression.",
                                )
                                .small(),
                            );

                            egui::ComboBox::from_label("Voice Tail")
                                .selected_text(match self.config.audio.mic_ns_hangover_frames {
                                    0..=5 => "Short",
                                    6..=15 => "Medium",
                                    _ => "Long",
                                })
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(
                                        &mut self.config.audio.mic_ns_hangover_frames,
                                        2,
                                        "Short",
                                    );
                                    ui.selectable_value(
                                        &mut self.config.audio.mic_ns_hangover_frames,
                                        10,
                                        "Medium",
                                    );
                                    ui.selectable_value(
                                        &mut self.config.audio.mic_ns_hangover_frames,
                                        25,
                                        "Long",
                                    );
                                });
                            ui.label(
                                egui::RichText::new(
                                    "How long the gate stays open after you finish speaking.",
                                )
                                .small(),
                            );

                            if ui.link("Reset to Defaults").clicked() {
                                self.config.audio.mic_ns_min_gain_percent = 1;
                                self.config.audio.mic_ns_vad_noise_threshold_percent = 25;
                                self.config.audio.mic_ns_vad_gate_threshold_percent = 55;
                                self.config.audio.mic_ns_snr_min_tenths = 12;
                                self.config.audio.mic_ns_snr_max_tenths = 60;
                                self.config.audio.mic_ns_hangover_frames = 10;
                                self.config.audio.mic_ns_noise_floor_fast_percent = 10;
                                self.config.audio.mic_ns_noise_floor_slow_percent = 1;
                                self.config.audio.mic_ns_attack_ms = 1;
                                self.config.audio.mic_ns_release_ms = 30;
                            }
                        });
                    }

                    ui.add_space(10.0);
                    ui.separator();
                    ui.add_space(10.0);

                    ui.add(
                        egui::Slider::new(&mut self.config.audio.master_volume, 0..=200)
                            .text("Master Volume %"),
                    );

                    ui.add(
                        egui::Slider::new(&mut self.config.audio.balance, -100..=100)
                            .text("Stereo Balance"),
                    );

                    ui.add_space(10.0);
                    ui.checkbox(
                        &mut self.config.audio.compression_enabled,
                        "Enable Compression",
                    );

                    if self.config.audio.compression_enabled {
                        ui.indent("compression_settings", |ui| {
                            ui.add(
                                egui::Slider::new(
                                    &mut self.config.audio.compression_threshold,
                                    0..=100,
                                )
                                .text("Compression Threshold"),
                            );
                            ui.add(
                                egui::Slider::new(&mut self.config.audio.compression_ratio, 1..=20)
                                    .text("Compression Ratio"),
                            );
                            ui.add(
                                egui::Slider::new(
                                    &mut self.config.audio.compression_attack,
                                    1..=100,
                                )
                                .text("Attack Time (ms)"),
                            );
                            ui.add(
                                egui::Slider::new(
                                    &mut self.config.audio.compression_release,
                                    50..=255,
                                )
                                .text("Release Time (ms)"),
                            );
                        });
                    }
                });

                // Hotkey Settings
                ui.collapsing("Hotkeys", |ui| {
                    render_hotkey_field(
                        ui,
                        "Save Clip:",
                        &mut self.config.hotkeys.save_clip,
                        &mut self.hotkey_errors.save_clip,
                    );
                    render_hotkey_field(
                        ui,
                        "Toggle Recording:",
                        &mut self.config.hotkeys.toggle_recording,
                        &mut self.hotkey_errors.toggle_recording,
                    );
                    render_hotkey_field(
                        ui,
                        "Screenshot:",
                        &mut self.config.hotkeys.screenshot,
                        &mut self.hotkey_errors.screenshot,
                    );
                    render_hotkey_field(
                        ui,
                        "Open Clip & Compress:",
                        &mut self.config.hotkeys.open_gallery,
                        &mut self.hotkey_errors.open_gallery,
                    );
                });

                // Advanced Settings
                ui.collapsing("Advanced", |ui| {
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
                });
            });

            ui.add_space(10.0);
            ui.separator();
            ui.add_space(5.0);

            ui.horizontal(|ui| {
                if ui.button("Apply").clicked() {
                    self.config.validate();
                    match self
                        .event_tx
                        .try_send(AppEvent::ConfigUpdated(Arc::new(self.config.clone())))
                    {
                        Ok(_) => {
                            *is_open = false;
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                        Err(e) => {
                            self.save_status = Some(format!("Error: {}", e));
                        }
                    }
                }

                if ui.button("Cancel").clicked() {
                    *is_open = false;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
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
