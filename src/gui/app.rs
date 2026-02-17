//! Main GUI application for LiteClip Replay settings

use anyhow::Result;
use tracing::{error, info};
use winit::platform::windows::EventLoopBuilderExtWindows;

use crate::config::{
    Codec, Config, EncoderType, OverlayPosition, QualityPreset, RateControl, Resolution,
};

/// Result from running the settings GUI
///
/// Contains information about what actions were taken in the settings window
/// and whether a restart is required for changes to take effect.
#[derive(Debug, Clone)]
pub struct GuiResult {
    /// Whether the configuration was saved
    pub saved: bool,
    /// Whether a restart is required for some changes to take effect
    pub restart_required: bool,
    /// The new configuration if it was saved, None otherwise
    pub new_config: Option<Config>,
}

impl GuiResult {
    /// Create a new GuiResult indicating no changes were saved
    pub fn unchanged() -> Self {
        Self {
            saved: false,
            restart_required: false,
            new_config: None,
        }
    }

    /// Create a new GuiResult indicating changes were saved
    pub fn saved(config: Config, restart_required: bool) -> Self {
        Self {
            saved: true,
            restart_required,
            new_config: Some(config),
        }
    }
}

/// Settings application state
///
/// Manages the configuration editor with support for dirty tracking,
/// cancel functionality, and restart detection.
pub struct SettingsApp {
    /// The configuration being edited
    config: Config,
    /// Original configuration for cancel functionality
    original_config: Config,
    /// Whether changes have been made
    dirty: bool,
    /// Whether changes require application restart
    restart_required: bool,
    /// Currently selected settings tab
    selected_tab: SettingsTab,
    /// Sender for the result when the window closes
    result_tx: Option<std::sync::mpsc::Sender<GuiResult>>,
    /// Whether the configuration was saved at least once
    was_saved: bool,
    /// Last save error message (if any)
    save_error: Option<String>,
    /// Whether save is in progress
    save_in_progress: bool,
}

/// Settings categories for navigation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SettingsTab {
    General,
    Video,
    Audio,
    Hotkeys,
    Advanced,
}

impl SettingsTab {
    /// Get all tabs in order
    fn all() -> &'static [SettingsTab] {
        &[
            SettingsTab::General,
            SettingsTab::Video,
            SettingsTab::Audio,
            SettingsTab::Hotkeys,
            SettingsTab::Advanced,
        ]
    }

    /// Display name for the tab
    fn name(&self) -> &'static str {
        match self {
            SettingsTab::General => "General",
            SettingsTab::Video => "Video",
            SettingsTab::Audio => "Audio",
            SettingsTab::Hotkeys => "Hotkeys",
            SettingsTab::Advanced => "Advanced",
        }
    }
}

impl SettingsApp {
    /// Create a new settings application with the given configuration
    pub fn new(config: Config, result_tx: std::sync::mpsc::Sender<GuiResult>) -> Self {
        Self {
            original_config: config.clone(),
            config,
            dirty: false,
            restart_required: false,
            selected_tab: SettingsTab::General,
            result_tx: Some(result_tx),
            was_saved: false,
            save_error: None,
            save_in_progress: false,
        }
    }

    /// Show the settings window
    pub fn show(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            // Top heading
            ui.heading("LiteClip Replay Settings");
            ui.add_space(16.0);

            // Main layout: navigation on left, content on right
            egui::SidePanel::left("settings_nav")
                .resizable(false)
                .min_width(120.0)
                .max_width(120.0)
                .show_inside(ui, |ui| {
                    self.show_navigation(ui);
                });

            egui::CentralPanel::default().show_inside(ui, |ui| {
                ui.set_min_width(400.0);
                self.show_settings_content(ui);
            });
        });

        // Bottom panel with save/reset buttons
        egui::TopBottomPanel::bottom("action_panel")
            .min_height(60.0)
            .show(ctx, |ui| {
                self.show_action_panel(ui);
            });
    }

    /// Show left-side navigation panel
    fn show_navigation(&mut self, ui: &mut egui::Ui) {
        ui.vertical(|ui| {
            ui.add_space(8.0);
            for tab in SettingsTab::all() {
                let is_selected = self.selected_tab == *tab;
                let button = egui::Button::new(tab.name())
                    .min_size(egui::vec2(100.0, 32.0))
                    .fill(if is_selected {
                        ui.visuals().selection.bg_fill
                    } else {
                        ui.visuals().widgets.inactive.bg_fill
                    });

                if ui.add(button).clicked() {
                    self.selected_tab = *tab;
                }
                ui.add_space(4.0);
            }
        });
    }

    /// Show settings content based on selected tab
    fn show_settings_content(&mut self, ui: &mut egui::Ui) {
        ui.set_min_width(400.0);

        match self.selected_tab {
            SettingsTab::General => self.show_general_settings(ui),
            SettingsTab::Video => self.show_video_settings(ui),
            SettingsTab::Audio => self.show_audio_settings(ui),
            SettingsTab::Hotkeys => self.show_hotkey_settings(ui),
            SettingsTab::Advanced => self.show_advanced_settings(ui),
        }
    }

    /// Show General settings panel
    fn show_general_settings(&mut self, ui: &mut egui::Ui) {
        ui.heading("General Settings");
        ui.add_space(16.0);

        // Replay duration
        ui.horizontal(|ui| {
            ui.label("Replay Duration:");
            let response = ui.add(
                egui::Slider::new(&mut self.config.general.replay_duration_secs, 30..=600)
                    .text("seconds"),
            );
            if response.changed() {
                self.mark_dirty();
            }
            if ui
                .button("↺")
                .on_hover_text("Reset to default (120s)")
                .clicked()
            {
                self.config.general.replay_duration_secs = 120;
                self.mark_dirty();
            }
        });
        ui.add_space(8.0);

        // Save directory
        ui.group(|ui| {
            ui.label("Save Directory:");
            let response = ui.text_edit_singleline(&mut self.config.general.save_directory);
            if response.changed() {
                self.mark_dirty();
            }
        });
        ui.add_space(8.0);

        // Boolean toggles
        let mut changed = false;
        changed |= ui
            .checkbox(
                &mut self.config.general.auto_start_with_windows,
                "Auto-start with Windows",
            )
            .changed();
        changed |= ui
            .checkbox(&mut self.config.general.start_minimised, "Start minimized")
            .changed();
        changed |= ui
            .checkbox(
                &mut self.config.general.notifications,
                "Enable notifications",
            )
            .changed();
        changed |= ui
            .checkbox(
                &mut self.config.general.auto_detect_game,
                "Auto-detect games",
            )
            .changed();

        if changed {
            self.mark_dirty();
        }
    }

    /// Show Video settings panel
    fn show_video_settings(&mut self, ui: &mut egui::Ui) {
        ui.heading("Video Settings");
        ui.add_space(16.0);

        // Resolution (requires restart)
        ui.horizontal(|ui| {
            ui.label("Resolution:");
            self.show_restart_indicator(ui);
            egui::ComboBox::from_id_salt("resolution_combo")
                .selected_text(format!("{:?}", self.config.video.resolution))
                .show_ui(ui, |ui| {
                    let old_value = self.config.video.resolution;
                    if (ui
                        .selectable_value(
                            &mut self.config.video.resolution,
                            Resolution::Native,
                            "Native",
                        )
                        .clicked()
                        || ui
                            .selectable_value(
                                &mut self.config.video.resolution,
                                Resolution::P1080,
                                "1080p",
                            )
                            .clicked()
                        || ui
                            .selectable_value(
                                &mut self.config.video.resolution,
                                Resolution::P720,
                                "720p",
                            )
                            .clicked()
                        || ui
                            .selectable_value(
                                &mut self.config.video.resolution,
                                Resolution::P480,
                                "480p",
                            )
                            .clicked())
                        && old_value != self.config.video.resolution
                    {
                        self.mark_dirty_with_restart();
                    }
                });
        });
        ui.add_space(8.0);

        // Framerate
        ui.horizontal(|ui| {
            ui.label("Framerate:");
            let response =
                ui.add(egui::Slider::new(&mut self.config.video.framerate, 1..=240).text("fps"));
            if response.changed() {
                self.mark_dirty();
            }
        });
        ui.add_space(8.0);

        // Codec (requires restart)
        ui.horizontal(|ui| {
            ui.label("Codec:");
            self.show_restart_indicator(ui);
            egui::ComboBox::from_id_salt("codec_combo")
                .selected_text(format!("{:?}", self.config.video.codec))
                .show_ui(ui, |ui| {
                    let old_value = self.config.video.codec;
                    if (ui
                        .selectable_value(&mut self.config.video.codec, Codec::H264, "H.264")
                        .clicked()
                        || ui
                            .selectable_value(
                                &mut self.config.video.codec,
                                Codec::H265,
                                "H.265 (HEVC)",
                            )
                            .clicked()
                        || ui
                            .selectable_value(&mut self.config.video.codec, Codec::Av1, "AV1")
                            .clicked())
                        && old_value != self.config.video.codec
                    {
                        self.mark_dirty_with_restart();
                    }
                });
        });
        ui.add_space(8.0);

        // Bitrate
        ui.horizontal(|ui| {
            ui.label("Bitrate:");
            let response = ui
                .add(egui::Slider::new(&mut self.config.video.bitrate_mbps, 1..=500).text("Mbps"));
            if response.changed() {
                self.mark_dirty();
            }
        });
        ui.add_space(8.0);

        // Encoder (requires restart)
        ui.horizontal(|ui| {
            ui.label("Encoder:");
            self.show_restart_indicator(ui);
            egui::ComboBox::from_id_salt("encoder_combo")
                .selected_text(format!("{:?}", self.config.video.encoder))
                .show_ui(ui, |ui| {
                    let old_value = self.config.video.encoder;
                    if (ui
                        .selectable_value(&mut self.config.video.encoder, EncoderType::Auto, "Auto")
                        .clicked()
                        || ui
                            .selectable_value(
                                &mut self.config.video.encoder,
                                EncoderType::Nvenc,
                                "NVENC (NVIDIA)",
                            )
                            .clicked()
                        || ui
                            .selectable_value(
                                &mut self.config.video.encoder,
                                EncoderType::Amf,
                                "AMF (AMD)",
                            )
                            .clicked()
                        || ui
                            .selectable_value(
                                &mut self.config.video.encoder,
                                EncoderType::Qsv,
                                "QSV (Intel)",
                            )
                            .clicked()
                        || ui
                            .selectable_value(
                                &mut self.config.video.encoder,
                                EncoderType::Software,
                                "Software",
                            )
                            .clicked())
                        && old_value != self.config.video.encoder
                    {
                        self.mark_dirty_with_restart();
                    }
                });
        });
        ui.add_space(8.0);

        // Quality preset
        ui.horizontal(|ui| {
            ui.label("Quality Preset:");
            egui::ComboBox::from_id_salt("quality_preset_combo")
                .selected_text(format!("{:?}", self.config.video.quality_preset))
                .show_ui(ui, |ui| {
                    let old_value = self.config.video.quality_preset;
                    if (ui
                        .selectable_value(
                            &mut self.config.video.quality_preset,
                            QualityPreset::Performance,
                            "Performance",
                        )
                        .clicked()
                        || ui
                            .selectable_value(
                                &mut self.config.video.quality_preset,
                                QualityPreset::Balanced,
                                "Balanced",
                            )
                            .clicked()
                        || ui
                            .selectable_value(
                                &mut self.config.video.quality_preset,
                                QualityPreset::Quality,
                                "Quality",
                            )
                            .clicked())
                        && old_value != self.config.video.quality_preset
                    {
                        self.mark_dirty();
                    }
                });
        });
        ui.add_space(8.0);

        // Rate control
        ui.horizontal(|ui| {
            ui.label("Rate Control:");
            egui::ComboBox::from_id_salt("rate_control_combo")
                .selected_text(format!("{:?}", self.config.video.rate_control))
                .show_ui(ui, |ui| {
                    let old_value = self.config.video.rate_control;
                    let cq_selected = ui
                        .selectable_value(
                            &mut self.config.video.rate_control,
                            RateControl::Cq,
                            "CQ (Constant Quality)",
                        )
                        .clicked();
                    let cbr_selected = ui
                        .selectable_value(
                            &mut self.config.video.rate_control,
                            RateControl::Cbr,
                            "CBR (Constant Bitrate)",
                        )
                        .clicked();
                    let vbr_selected = ui
                        .selectable_value(
                            &mut self.config.video.rate_control,
                            RateControl::Vbr,
                            "VBR (Variable Bitrate)",
                        )
                        .clicked();

                    if (cq_selected || cbr_selected || vbr_selected)
                        && old_value != self.config.video.rate_control
                    {
                        self.mark_dirty();
                        // Auto-set quality value if switching to CQ
                        if matches!(self.config.video.rate_control, RateControl::Cq)
                            && self.config.video.quality_value.is_none()
                        {
                            self.config.video.quality_value = Some(23);
                        }
                    }
                });
        });
        ui.add_space(8.0);

        // Quality value (only for CQ mode)
        if matches!(self.config.video.rate_control, RateControl::Cq) {
            let mut quality = self.config.video.quality_value.unwrap_or(23);
            ui.horizontal(|ui| {
                ui.label("Quality Value:");
                let response =
                    ui.add(egui::Slider::new(&mut quality, 1..=51).text("(lower is better)"));
                if response.changed() {
                    self.config.video.quality_value = Some(quality);
                    self.mark_dirty();
                }
            });
            ui.add_space(8.0);
        }
    }

    /// Show Audio settings panel
    fn show_audio_settings(&mut self, ui: &mut egui::Ui) {
        ui.heading("Audio Settings");
        ui.add_space(16.0);

        // Warning about audio not fully implemented
        ui.colored_label(
            egui::Color32::YELLOW,
            "Note: Audio capture is experimental. Clips may not include audio.",
        );
        ui.add_space(8.0);

        // System audio
        ui.group(|ui| {
            let mut changed = false;
            changed |= ui
                .checkbox(
                    &mut self.config.audio.capture_system,
                    "Capture System Audio",
                )
                .changed();

            if self.config.audio.capture_system {
                ui.horizontal(|ui| {
                    ui.label("System Volume:");
                    let response = ui.add(
                        egui::Slider::new(&mut self.config.audio.system_volume, 0..=100).text("%"),
                    );
                    if response.changed() {
                        changed = true;
                    }
                });
            }

            if changed {
                self.mark_dirty();
            }
        });
        ui.add_space(8.0);

        // Microphone
        ui.group(|ui| {
            let mut changed = false;
            changed |= ui
                .checkbox(&mut self.config.audio.capture_mic, "Capture Microphone")
                .changed();

            if self.config.audio.capture_mic {
                ui.horizontal(|ui| {
                    ui.label("Mic Device:");
                    let response = ui.text_edit_singleline(&mut self.config.audio.mic_device);
                    if response.changed() {
                        changed = true;
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Mic Volume:");
                    let response = ui.add(
                        egui::Slider::new(&mut self.config.audio.mic_volume, 0..=100).text("%"),
                    );
                    if response.changed() {
                        changed = true;
                    }
                });
            }

            if changed {
                self.mark_dirty();
            }
        });
    }

    /// Show Hotkey settings panel
    fn show_hotkey_settings(&mut self, ui: &mut egui::Ui) {
        ui.heading("Hotkey Settings");
        ui.add_space(16.0);

        ui.label("Hotkey format: Modifier+Key (e.g., 'Alt+F9', 'Ctrl+Shift+R')");
        ui.add_space(8.0);

        // Work around borrow checker by extracting mutable references first
        let hotkeys = &mut self.config.hotkeys;
        let mut hotkey_changed = false;

        // Save clip hotkey
        ui.horizontal(|ui| {
            ui.label("Save Clip:");
            let response = ui.text_edit_singleline(&mut hotkeys.save_clip);
            if response.changed() {
                hotkey_changed = true;
            }
            if ui.button("Clear").clicked() {
                hotkeys.save_clip.clear();
                hotkey_changed = true;
            }
        });
        ui.add_space(8.0);

        // Toggle recording hotkey
        ui.horizontal(|ui| {
            ui.label("Toggle Recording:");
            let response = ui.text_edit_singleline(&mut hotkeys.toggle_recording);
            if response.changed() {
                hotkey_changed = true;
            }
            if ui.button("Clear").clicked() {
                hotkeys.toggle_recording.clear();
                hotkey_changed = true;
            }
        });
        ui.add_space(8.0);

        // Screenshot hotkey
        ui.horizontal(|ui| {
            ui.label("Screenshot:");
            let response = ui.text_edit_singleline(&mut hotkeys.screenshot);
            if response.changed() {
                hotkey_changed = true;
            }
            if ui.button("Clear").clicked() {
                hotkeys.screenshot.clear();
                hotkey_changed = true;
            }
        });
        ui.add_space(8.0);

        // Open gallery hotkey
        ui.horizontal(|ui| {
            ui.label("Open Gallery:");
            let response = ui.text_edit_singleline(&mut hotkeys.open_gallery);
            if response.changed() {
                hotkey_changed = true;
            }
            if ui.button("Clear").clicked() {
                hotkeys.open_gallery.clear();
                hotkey_changed = true;
            }
        });

        // Apply changes flag after closures complete
        if hotkey_changed {
            self.dirty = true;
        }
    }

    /// Show Advanced settings panel
    fn show_advanced_settings(&mut self, ui: &mut egui::Ui) {
        ui.heading("Advanced Settings");
        ui.add_space(16.0);

        ui.collapsing("Performance", |ui| {
            // Memory limit
            ui.horizontal(|ui| {
                ui.label("Memory Limit:");
                let response = ui.add(
                    egui::Slider::new(&mut self.config.advanced.memory_limit_mb, 512..=16384)
                        .text("MB"),
                );
                if response.changed() {
                    self.mark_dirty();
                }
            });
            ui.add_space(8.0);

            // GPU index (requires restart)
            ui.horizontal(|ui| {
                ui.label("GPU Index:");
                self.show_restart_indicator(ui);
                let mut gpu_idx = self.config.advanced.gpu_index as i32;
                let response = ui.add(egui::DragValue::new(&mut gpu_idx).range(0..=8));
                if response.changed() {
                    self.config.advanced.gpu_index = gpu_idx as u32;
                    self.mark_dirty_with_restart();
                }
            });
            ui.add_space(8.0);

            // CPU readback (requires restart)
            ui.horizontal(|ui| {
                let old_value = self.config.advanced.use_cpu_readback;
                let response = ui.checkbox(
                    &mut self.config.advanced.use_cpu_readback,
                    "Use CPU Readback",
                );
                self.show_restart_indicator(ui);
                if response.changed() && old_value != self.config.advanced.use_cpu_readback {
                    self.mark_dirty_with_restart();
                }
            });
        });
        ui.add_space(8.0);

        ui.collapsing("Recording", |ui| {
            // Keyframe interval
            ui.horizontal(|ui| {
                ui.label("Keyframe Interval:");
                let response = ui.add(
                    egui::Slider::new(&mut self.config.advanced.keyframe_interval_secs, 1..=10)
                        .text("seconds"),
                );
                if response.changed() {
                    self.mark_dirty();
                }
            });
        });
        ui.add_space(8.0);

        ui.collapsing("Overlay", |ui| {
            let mut changed = false;
            changed |= ui
                .checkbox(&mut self.config.advanced.overlay_enabled, "Enable Overlay")
                .changed();

            if self.config.advanced.overlay_enabled {
                ui.horizontal(|ui| {
                    ui.label("Overlay Position:");
                    egui::ComboBox::from_id_salt("overlay_pos_combo")
                        .selected_text(format!("{:?}", self.config.advanced.overlay_position))
                        .show_ui(ui, |ui| {
                            let old_value = self.config.advanced.overlay_position;
                            if (ui
                                .selectable_value(
                                    &mut self.config.advanced.overlay_position,
                                    OverlayPosition::TopLeft,
                                    "Top Left",
                                )
                                .clicked()
                                || ui
                                    .selectable_value(
                                        &mut self.config.advanced.overlay_position,
                                        OverlayPosition::TopRight,
                                        "Top Right",
                                    )
                                    .clicked()
                                || ui
                                    .selectable_value(
                                        &mut self.config.advanced.overlay_position,
                                        OverlayPosition::BottomLeft,
                                        "Bottom Left",
                                    )
                                    .clicked()
                                || ui
                                    .selectable_value(
                                        &mut self.config.advanced.overlay_position,
                                        OverlayPosition::BottomRight,
                                        "Bottom Right",
                                    )
                                    .clicked())
                                && old_value != self.config.advanced.overlay_position
                            {
                                changed = true;
                            }
                        });
                });
            }

            if changed {
                self.mark_dirty();
            }
        });
    }

    /// Show restart indicator badge
    fn show_restart_indicator(&self, ui: &mut egui::Ui) {
        ui.label(
            egui::RichText::new("* Requires restart")
                .color(ui.visuals().warn_fg_color)
                .small(),
        );
    }

    /// Show bottom action panel with save/reset buttons
    fn show_action_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            // Save button - disabled if dirty or save in progress
            let save_button = egui::Button::new("Save").min_size(egui::vec2(80.0, 32.0));
            let can_save = self.dirty && !self.save_in_progress;
            let save_response = ui.add_enabled(can_save, save_button);

            if save_response.clicked() {
                // Validate config before saving
                self.config.validate();
                self.save_in_progress = true;
                self.save_error = None;

                // Use channel to get result from spawned task
                let (tx, rx) = std::sync::mpsc::channel();
                let config = self.config.clone();

                tokio::spawn(async move {
                    let result = config.save().await;
                    let _ = tx.send(result);
                });

                // Check result immediately (will be ready on next frame)
                match rx.try_recv() {
                    Ok(Ok(())) => {
                        self.original_config = self.config.clone();
                        self.dirty = false;
                        self.was_saved = true;
                        self.save_in_progress = false;
                        info!("Configuration saved successfully");
                    }
                    Ok(Err(e)) => {
                        self.save_error = Some(e.to_string());
                        self.save_in_progress = false;
                        error!("Failed to save config: {}", self.save_error.as_ref().unwrap());
                    }
                    Err(_) => {
                        // Result not ready yet - will check on next frame
                    }
                }
            }

            // Reset button
            if ui.button("Reset").clicked() {
                self.reset();
            }

            ui.separator();

            // Status text
            if self.save_in_progress {
                ui.label("Saving...");
            } else if let Some(ref err) = self.save_error {
                ui.colored_label(egui::Color32::RED, &format!("Save failed: {}", err));
            } else if self.dirty {
                if self.restart_required {
                    ui.colored_label(
                        ui.visuals().warn_fg_color,
                        "Changes saved. Restart required for some settings to take effect.",
                    );
                } else {
                    ui.label("Unsaved changes");
                }
            } else if self.restart_required {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    "Restart required for changes to take effect.",
                );
            } else {
                ui.label("All changes saved");
            }
        });
    }

    /// Save the configuration to disk
    ///
    /// Returns true if save was successful
    pub async fn save(&mut self) -> Result<bool> {
        // Validate before saving
        self.config.validate();

        match self.config.save().await {
            Ok(()) => {
                self.original_config = self.config.clone();
                self.dirty = false;
                self.was_saved = true;
                info!("Configuration saved successfully");
                Ok(true)
            }
            Err(e) => {
                error!("Failed to save configuration: {}", e);
                Err(e)
            }
        }
    }

    /// Reset configuration to original values
    pub fn reset(&mut self) {
        self.config = self.original_config.clone();
        self.dirty = false;
        self.restart_required = false;
        info!("Configuration reset to original values");
    }

    /// Check if changes have been made
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Check if restart is required for changes to take effect
    pub fn is_restart_required(&self) -> bool {
        self.restart_required
    }

    /// Get the current configuration
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Get whether the configuration was saved at least once
    pub fn was_saved(&self) -> bool {
        self.was_saved
    }

    /// Send the result when the window closes
    fn send_result(&mut self) {
        if let Some(tx) = self.result_tx.take() {
            let result = if self.was_saved {
                GuiResult::saved(self.config.clone(), self.restart_required)
            } else {
                GuiResult::unchanged()
            };
            let _ = tx.send(result);
        }
    }

    /// Mark the configuration as dirty
    fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// Mark dirty and set restart required
    fn mark_dirty_with_restart(&mut self) {
        self.dirty = true;
        self.restart_required = true;
    }
}

impl eframe::App for SettingsApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.show(ctx);

        // Check if the window was requested to close
        if ctx.input(|i| i.viewport().close_requested()) {
            self.send_result();
        }
    }
}

/// Run the settings window
///
/// This function blocks until the window is closed.
/// Returns a [`GuiResult`] indicating whether changes were saved and if a restart is required.
pub fn run_settings_window(config: Config) -> Result<GuiResult> {
    let (result_tx, result_rx) = std::sync::mpsc::channel();

    // Use Windows-specific extension to allow event loop on any thread
    // This is required because the GUI may be spawned from tokio's blocking thread pool
    let event_loop_builder = Box::new(
        |builder: &mut winit::event_loop::EventLoopBuilder<eframe::UserEvent>| {
            builder.with_any_thread(true);
        },
    )
        as Box<dyn FnMut(&mut winit::event_loop::EventLoopBuilder<eframe::UserEvent>) + Send>;

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([800.0, 600.0])
            .with_min_inner_size([600.0, 400.0]),
        event_loop_builder: Some(event_loop_builder),
        ..Default::default()
    };

    eframe::run_native(
        "LiteClip Replay Settings",
        options,
        Box::new(move |_cc| {
            let app = SettingsApp::new(config, result_tx);
            Ok(Box::new(app) as Box<dyn eframe::App>)
        }),
    )
    .map_err(|e| anyhow::anyhow!("Failed to run settings window: {}", e))?;

    // Receive the result from the channel
    let result = result_rx.recv().unwrap_or_else(|_| {
        // If we can't receive (sender dropped without sending), return unchanged
        GuiResult::unchanged()
    });

    Ok(result)
}

/// Run the settings window and get the result
///
/// This is an async wrapper that runs the GUI in a blocking task and returns
/// the result including whether changes were saved and if restart is required.
pub async fn run_settings_window_async(config: Config) -> Result<GuiResult> {
    tokio::task::spawn_blocking(move || run_settings_window(config))
        .await
        .map_err(|e| anyhow::anyhow!("GUI task panicked: {:?}", e))?
}
