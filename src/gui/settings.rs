use eframe::egui;
use std::sync::Arc;
use tokio::sync::mpsc::Sender;

use crate::capture::audio::AudioLevelMonitor;
use crate::capture::detect_display_resolution;
use crate::config::{config_mod::types::*, Config};
use crate::config::{
    MAX_REPLAY_MEMORY_LIMIT_MB, MIN_REPLAY_MEMORY_LIMIT_MB, REPLAY_MEMORY_LIMIT_AUTO_MB,
};
use crate::platform::AppEvent;

pub fn show_settings_gui(
    event_tx: Sender<AppEvent>,
    level_monitor: Option<AudioLevelMonitor>,
    config: Config,
) {
    crate::gui::manager::send_gui_message(crate::gui::manager::GuiMessage::ShowSettings(
        event_tx,
        level_monitor,
        config,
    ));
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

use crate::capture::audio::level_monitor::AudioLevels;

fn render_audio_level_meter(ui: &mut egui::Ui, active: bool, levels: AudioLevels) {
    let meter_width = 160.0;
    let meter_height = 8.0;
    let rounding = 4.0;

    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(meter_width, meter_height), egui::Sense::hover());

    if ui.is_rect_visible(rect) {
        let painter = ui.painter();

        painter.rect_filled(rect, rounding, egui::Color32::from_rgb(30, 30, 30));

        if active && levels.level > 0 {
            let level_frac = levels.level as f32 / 100.0;
            let filled_width = level_frac * meter_width;

            let level_color = if levels.level >= 95 {
                egui::Color32::from_rgb(0xED, 0x42, 0x42)
            } else if levels.level >= 80 {
                egui::Color32::from_rgb(0xFA, 0xA6, 0x1A)
            } else if levels.level >= 50 {
                egui::Color32::from_rgb(0xF0, 0xB2, 0x32)
            } else {
                egui::Color32::from_rgb(0x23, 0xA5, 0x5A)
            };

            let filled_rect = egui::Rect::from_min_size(
                rect.min,
                egui::vec2(filled_width.min(meter_width), meter_height),
            );
            painter.rect_filled(filled_rect, rounding, level_color);

            if levels.peak > levels.level {
                let peak_frac = levels.peak.min(100) as f32 / 100.0;
                let peak_x = rect.min.x + peak_frac * meter_width - 2.0;
                let peak_rect = egui::Rect::from_min_size(
                    egui::pos2(peak_x.max(rect.min.x), rect.min.y),
                    egui::vec2(3.0, meter_height),
                );
                painter.rect_filled(peak_rect, 1.0, egui::Color32::WHITE);
            }
        }
    }
}

const SIDEBAR_WIDTH: f32 = 120.0;

#[derive(Default, PartialEq, Eq, Clone, Copy)]
enum SettingsTab {
    #[default]
    General,
    Video,
    Audio,
    Hotkeys,
    Advanced,
}

impl SettingsTab {
    fn label(&self) -> &'static str {
        match self {
            SettingsTab::General => "General",
            SettingsTab::Video => "Video",
            SettingsTab::Audio => "Audio",
            SettingsTab::Hotkeys => "Hotkeys",
            SettingsTab::Advanced => "Advanced",
        }
    }

    fn all() -> [SettingsTab; 5] {
        [
            SettingsTab::General,
            SettingsTab::Video,
            SettingsTab::Audio,
            SettingsTab::Hotkeys,
            SettingsTab::Advanced,
        ]
    }
}

pub struct SettingsApp {
    pub config: Config,
    pub event_tx: Sender<AppEvent>,
    pub save_status: Option<String>,
    hotkey_errors: HotkeyValidationErrors,
    level_monitor: Option<AudioLevelMonitor>,
    last_audio_levels: Option<(AudioLevels, AudioLevels)>,
    mic_devices: Vec<(String, String)>,
    current_tab: SettingsTab,
    last_tab: SettingsTab,
}

impl SettingsApp {
    pub fn new(
        config: Config,
        event_tx: Sender<AppEvent>,
        level_monitor: Option<AudioLevelMonitor>,
    ) -> Self {
        let mic_devices = crate::capture::audio::device_info::list_capture_devices();

        Self {
            config,
            event_tx,
            save_status: None,
            hotkey_errors: HotkeyValidationErrors::default(),
            level_monitor,
            last_audio_levels: None,
            mic_devices,
            current_tab: SettingsTab::default(),
            last_tab: SettingsTab::default(),
        }
    }

    pub fn update(&mut self, ctx: &egui::Context, is_open: &mut bool) {
        self.render(ctx, is_open);
    }

    pub fn release_resources(&mut self) {
        self.save_status = None;
        self.level_monitor = None;
        self.last_audio_levels = None;
    }

    /// Refresh microphone device list from system
    fn refresh_mic_devices(&mut self) {
        self.mic_devices = crate::capture::audio::device_info::list_capture_devices();
    }

    fn render(&mut self, ctx: &egui::Context, is_open: &mut bool) {
        // Refresh microphone devices once when switching to Audio tab
        if self.current_tab == SettingsTab::Audio && self.last_tab != SettingsTab::Audio {
            self.refresh_mic_devices();
        }
        self.last_tab = self.current_tab;

        // Render sidebar on the left
        egui::SidePanel::left("settings_sidebar")
            .exact_width(SIDEBAR_WIDTH)
            .resizable(false)
            .show(ctx, |ui| {
                self.render_sidebar(ui);
            });

        // Render main content in central panel
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::Frame::NONE
                .inner_margin(egui::Margin::same(8))
                .show(ui, |ui| {
                    // Header
                    ui.horizontal(|ui| {
                        ui.heading("LiteClip Settings");
                        ui.label(egui::RichText::new("—").weak());
                        ui.label(egui::RichText::new(self.current_tab.label()).strong());
                    });
                    ui.separator();

                    // Content area with scrolling
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| match self.current_tab {
                            SettingsTab::General => self.render_general_settings(ui),
                            SettingsTab::Video => self.render_video_settings(ui),
                            SettingsTab::Audio => self.render_audio_settings(ui),
                            SettingsTab::Hotkeys => self.render_hotkeys_settings(ui),
                            SettingsTab::Advanced => self.render_advanced_settings(ui),
                        });
                });

            // Bottom button bar
            ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                ui.add_space(5.0);
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
        });
    }

    fn render_sidebar(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(8.0);
            ui.heading("Settings");
            ui.add_space(16.0);

            for tab in SettingsTab::all() {
                let is_selected = self.current_tab == tab;
                let button_text = egui::RichText::new(tab.label()).size(14.0);

                let button = if is_selected {
                    egui::Button::new(button_text.strong())
                        .fill(egui::Color32::from_rgb(60, 60, 70))
                        .stroke(egui::Stroke::new(
                            1.0,
                            egui::Color32::from_rgb(100, 100, 120),
                        ))
                        .min_size(egui::vec2(100.0, 28.0))
                } else {
                    egui::Button::new(button_text)
                        .fill(egui::Color32::TRANSPARENT)
                        .stroke(egui::Stroke::NONE)
                        .min_size(egui::vec2(100.0, 28.0))
                };

                if ui.add(button).clicked() {
                    self.current_tab = tab;
                }
                ui.add_space(4.0);
            }
        });
    }

    fn render_general_settings(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Save Directory:");
            ui.text_edit_singleline(&mut self.config.general.save_directory);
            if ui.button("Browse...").clicked() {
                if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                    self.config.general.save_directory = folder.to_string_lossy().to_string();
                }
            }
        });

        ui.add_space(8.0);

        ui.add(
            egui::Slider::new(&mut self.config.general.replay_duration_secs, 5..=300)
                .text("Replay Duration (s)"),
        );

        ui.add_space(8.0);

        ui.checkbox(
            &mut self.config.general.auto_start_with_windows,
            "Auto Start with Windows",
        );
        ui.checkbox(&mut self.config.general.start_minimised, "Start Minimised");
        ui.checkbox(
            &mut self.config.general.auto_detect_game,
            "Auto Detect Game",
        );
        ui.checkbox(
            &mut self.config.general.generate_clip_thumbnail,
            "Generate clip thumbnail after save",
        );
        ui.label(
            egui::RichText::new(
                "Turn off to A/B memory after save, or set LITECLIP_SKIP_THUMBNAIL=1.",
            )
            .small()
            .weak(),
        );

        ui.add_space(8.0);
        ui.separator();
        ui.label(egui::RichText::new("Clip export").strong());
        ui.checkbox(
            &mut self.config.general.use_software_encoder,
            "Use software encoder for export (slower, more compatible)",
        );
    }

    fn render_video_settings(&mut self, ui: &mut egui::Ui) {
        ui.checkbox(
            &mut self.config.video.use_native_resolution,
            "Use Native Resolution",
        );

        if !self.config.video.use_native_resolution {
            let (show_4k, show_1440p, show_ultrawide, show_super_ultrawide) =
                get_display_resolution_options();

            egui::ComboBox::from_label("Resolution")
                .selected_text(resolution_display_name(&self.config.video.resolution))
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut self.config.video.resolution,
                        Resolution::Native,
                        "Native (Desktop Resolution)",
                    );
                    ui.separator();
                    ui.label("Standard 16:9");
                    if show_4k {
                        ui.selectable_value(
                            &mut self.config.video.resolution,
                            Resolution::P2160,
                            "4K (3840x2160)",
                        );
                    }
                    if show_1440p {
                        ui.selectable_value(
                            &mut self.config.video.resolution,
                            Resolution::P1440,
                            "1440p (2560x1440)",
                        );
                    }
                    ui.selectable_value(
                        &mut self.config.video.resolution,
                        Resolution::P1080,
                        "1080p (1920x1080)",
                    );
                    ui.selectable_value(
                        &mut self.config.video.resolution,
                        Resolution::P720,
                        "720p (1280x720)",
                    );
                    ui.selectable_value(
                        &mut self.config.video.resolution,
                        Resolution::P480,
                        "480p (854x480)",
                    );

                    if show_ultrawide {
                        ui.separator();
                        ui.label("Ultrawide 21:9");
                        if show_4k {
                            ui.selectable_value(
                                &mut self.config.video.resolution,
                                Resolution::UW2160,
                                "UW 4K (5120x2160)",
                            );
                        }
                        ui.selectable_value(
                            &mut self.config.video.resolution,
                            Resolution::UW1440,
                            "UW 1440p (3440x1440)",
                        );
                        ui.selectable_value(
                            &mut self.config.video.resolution,
                            Resolution::UW1080,
                            "UW 1080p (2560x1080)",
                        );
                    }

                    if show_super_ultrawide {
                        ui.separator();
                        ui.label("Super Ultrawide 32:9");
                        if show_1440p {
                            ui.selectable_value(
                                &mut self.config.video.resolution,
                                Resolution::SuperUW1440,
                                "SUW 1440p (5120x1440)",
                            );
                        }
                        ui.selectable_value(
                            &mut self.config.video.resolution,
                            Resolution::SuperUW,
                            "SUW 1080p (3840x1080)",
                        );
                    }

                    ui.separator();
                    ui.label("Custom");
                    ui.selectable_value(
                        &mut self.config.video.resolution,
                        Resolution::Custom(1920, 1080),
                        "Custom Resolution",
                    );
                });

            // Show custom resolution input fields when Custom is selected
            if let Resolution::Custom(width, height) = &mut self.config.video.resolution {
                ui.horizontal(|ui| {
                    ui.label("Width:");
                    ui.add(egui::DragValue::new(width).range(160..=8192).speed(10.0));
                    ui.label("Height:");
                    ui.add(egui::DragValue::new(height).range(160..=8192).speed(10.0));
                });
                ui.label(
                    "Note: Dimensions will be rounded to even numbers for encoder compatibility",
                );
            }
        }

        ui.add_space(8.0);

        ui.add(egui::Slider::new(&mut self.config.video.framerate, 10..=144).text("Framerate"));
        ui.add(
            egui::Slider::new(&mut self.config.video.bitrate_mbps, 1..=150).text("Bitrate (Mbps)"),
        );

        ui.add_space(8.0);

        ui.label("Codec: HEVC (H.265)");

        ui.add_space(4.0);

        egui::ComboBox::from_label("Encoder")
            .selected_text(format!("{:?}", self.config.video.encoder))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut self.config.video.encoder, EncoderType::Auto, "Auto");
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

        ui.add_space(4.0);

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

        ui.add_space(4.0);

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
    }

    fn render_audio_settings(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.checkbox(
                &mut self.config.audio.capture_system,
                "Capture System Audio",
            );
            if let Some(ref monitor) = self.level_monitor {
                let levels = monitor.get_system_levels();
                render_audio_level_meter(ui, self.config.audio.capture_system, levels);
            }
        });

        ui.add(
            egui::Slider::new(&mut self.config.audio.system_volume, 0..=200)
                .text("System Volume %"),
        );

        ui.add_space(8.0);

        ui.horizontal(|ui| {
            ui.checkbox(&mut self.config.audio.capture_mic, "Capture Microphone");
            if let Some(ref monitor) = self.level_monitor {
                let levels = monitor.get_mic_levels();
                render_audio_level_meter(ui, self.config.audio.capture_mic, levels);
            }
        });

        ui.horizontal(|ui| {
            ui.label("Device:");
            let selected_device_name = self
                .mic_devices
                .iter()
                .find(|(_, id)| id == &self.config.audio.mic_device)
                .map(|(name, _)| name.clone())
                .unwrap_or_else(|| "Unavailable device (will fallback to default)".to_string());

            egui::ComboBox::from_id_salt("mic_device_combo")
                .selected_text(selected_device_name)
                .show_ui(ui, |ui| {
                    for (name, id) in &self.mic_devices {
                        ui.selectable_value(&mut self.config.audio.mic_device, id.clone(), name);
                    }
                });

            if ui.button("Refresh").clicked() {
                self.mic_devices = crate::capture::audio::device_info::list_capture_devices();
            }
        });

        if !self
            .mic_devices
            .iter()
            .any(|(_, id)| id == &self.config.audio.mic_device)
        {
            ui.label(
                egui::RichText::new(
                    "Selected microphone endpoint is unavailable; runtime capture will fallback to system default.",
                )
                .small()
                .weak(),
            );
        }

        ui.add(egui::Slider::new(&mut self.config.audio.mic_volume, 0..=400).text("Mic Volume %"));
        ui.checkbox(
            &mut self.config.audio.mic_noise_reduction,
            "Reduce mic background hiss",
        );
        ui.label(
            egui::RichText::new(
                "When enabled, high-quality AI-powered noise suppression removes background hiss and hum.",
            )
            .small(),
        );

        ui.add_space(8.0);
        ui.checkbox(
            &mut self.config.audio.normalization_enabled,
            "Gentle live audio normalization",
        );
        ui.label(
            egui::RichText::new(
                "Balances mic/system loudness and tracks overall program level slowly to preserve dynamics.",
            )
            .small(),
        );

        ui.add_enabled_ui(self.config.audio.normalization_enabled, |ui| {
            ui.add(
                egui::Slider::new(&mut self.config.audio.target_lufs, -23..=-14)
                    .text("Target Loudness (LUFS)"),
            );
        });

        ui.checkbox(
            &mut self.config.audio.true_peak_limiter_enabled,
            "True-peak safety limiter",
        );
        ui.add_enabled_ui(self.config.audio.true_peak_limiter_enabled, |ui| {
            ui.add(
                egui::Slider::new(&mut self.config.audio.true_peak_limit_dbtp, -3..=0)
                    .text("Limiter Ceiling (dBTP)"),
            );
        });

        ui.add_space(12.0);
        ui.separator();
        ui.add_space(12.0);

        ui.add(
            egui::Slider::new(&mut self.config.audio.master_volume, 0..=200)
                .text("Master Volume %"),
        );

        ui.add(
            egui::Slider::new(&mut self.config.audio.balance, -100..=100).text("Stereo Balance"),
        );
    }

    fn render_hotkeys_settings(&mut self, ui: &mut egui::Ui) {
        render_hotkey_field(
            ui,
            "Save Clip:",
            &mut self.config.hotkeys.save_clip,
            &mut self.hotkey_errors.save_clip,
        );

        ui.add_space(4.0);

        render_hotkey_field(
            ui,
            "Toggle Recording:",
            &mut self.config.hotkeys.toggle_recording,
            &mut self.hotkey_errors.toggle_recording,
        );

        ui.add_space(4.0);

        render_hotkey_field(
            ui,
            "Screenshot:",
            &mut self.config.hotkeys.screenshot,
            &mut self.hotkey_errors.screenshot,
        );

        ui.add_space(4.0);

        render_hotkey_field(
            ui,
            "Open Clip & Compress:",
            &mut self.config.hotkeys.open_gallery,
            &mut self.hotkey_errors.open_gallery,
        );
    }

    fn render_advanced_settings(&mut self, ui: &mut egui::Ui) {
        ui.add(egui::Slider::new(&mut self.config.advanced.gpu_index, 0..=4).text("GPU Index"));

        ui.add_space(4.0);

        ui.add(
            egui::Slider::new(&mut self.config.advanced.keyframe_interval_secs, 1..=10)
                .text("Keyframe Interval (s)"),
        );

        ui.add_space(4.0);

        ui.checkbox(
            &mut self.config.advanced.use_cpu_readback,
            "Use CPU Readback for HW Encoding",
        );

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(8.0);

        let estimated_mb = self.config.estimated_replay_storage_mb();
        let recommended_mb = self.config.recommended_replay_memory_limit_mb();
        let mut auto_memory_limit =
            self.config.advanced.memory_limit_mb == REPLAY_MEMORY_LIMIT_AUTO_MB;
        if ui
            .checkbox(
                &mut auto_memory_limit,
                format!(
                    "Auto replay memory limit (recommended {} MB)",
                    recommended_mb
                ),
            )
            .changed()
        {
            self.config.advanced.memory_limit_mb = if auto_memory_limit {
                REPLAY_MEMORY_LIMIT_AUTO_MB
            } else {
                recommended_mb.clamp(MIN_REPLAY_MEMORY_LIMIT_MB, MAX_REPLAY_MEMORY_LIMIT_MB)
            };
        }

        if !auto_memory_limit {
            ui.add(
                egui::Slider::new(
                    &mut self.config.advanced.memory_limit_mb,
                    MIN_REPLAY_MEMORY_LIMIT_MB..=MAX_REPLAY_MEMORY_LIMIT_MB,
                )
                .text("Replay Memory Limit (MB)"),
            );
        }

        let effective_mb = self.config.effective_replay_memory_limit_mb();
        ui.label(format!(
            "Replay estimate: {} MB, effective memory cap: {} MB",
            estimated_mb, effective_mb
        ));
    }
}

impl eframe::App for SettingsApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let mut _dummy = true;
        self.render(ctx, &mut _dummy);

        if self.level_monitor.is_some() && self.current_tab == SettingsTab::Audio {
            let mut request_ms = 220;
            if let Some(ref monitor) = self.level_monitor {
                let system = monitor.get_system_levels();
                let mic = monitor.get_mic_levels();
                let changed = self
                    .last_audio_levels
                    .map(|last| last != (system, mic))
                    .unwrap_or(true);
                self.last_audio_levels = Some((system, mic));

                if changed || system.level > 0 || mic.level > 0 || system.peak > 0 || mic.peak > 0 {
                    request_ms = 100;
                }
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(request_ms));
        }
    }
}

/// Returns a human-readable display name for a resolution variant.
fn resolution_display_name(resolution: &Resolution) -> String {
    match resolution {
        Resolution::Native => "Native (Desktop Resolution)".to_string(),
        Resolution::P480 => "480p (854x480)".to_string(),
        Resolution::P720 => "720p (1280x720)".to_string(),
        Resolution::P1080 => "1080p (1920x1080)".to_string(),
        Resolution::P1440 => "1440p (2560x1440)".to_string(),
        Resolution::P2160 => "4K (3840x2160)".to_string(),
        Resolution::UW1080 => "UW 1080p (2560x1080)".to_string(),
        Resolution::UW1440 => "UW 1440p (3440x1440)".to_string(),
        Resolution::UW2160 => "UW 4K (5120x2160)".to_string(),
        Resolution::SuperUW => "SUW 1080p (3840x1080)".to_string(),
        Resolution::SuperUW1440 => "SUW 1440p (5120x1440)".to_string(),
        Resolution::Custom(width, height) => format!("Custom ({}x{})", width, height),
    }
}

/// Determines which resolution options to show based on detected display capabilities.
///
/// Returns a tuple of (show_4k, show_1440p, show_ultrawide, show_super_ultrawide) booleans.
/// Uses the primary display (index 0) to determine appropriate options.
fn get_display_resolution_options() -> (bool, bool, bool, bool) {
    const MIN_WIDTH_1440P: u32 = 1920; // Show 1440p+ if display is at least 1920 wide
    const MIN_WIDTH_4K: u32 = 2560; // Show 4K if display is at least 2560 wide
    const MIN_WIDTH_ULTRAWIDE: u32 = 2560; // Show ultrawide if display is at least 2560 wide
    const MIN_WIDTH_SUPERUW: u32 = 3840; // Show super ultrawide if display is at least 3840 wide

    if let Some((width, _height)) = detect_display_resolution(0) {
        (
            width >= MIN_WIDTH_4K,        // Show 4K options
            width >= MIN_WIDTH_1440P,     // Show 1440p options
            width >= MIN_WIDTH_ULTRAWIDE, // Show ultrawide (21:9) options
            width >= MIN_WIDTH_SUPERUW,   // Show super ultrawide (32:9) options
        )
    } else {
        // If we can't detect the display, show all options (user might have exotic setup)
        (true, true, true, true)
    }
}
