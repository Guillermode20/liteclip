//! Clip-saved popup overlay
//!
//! A small, frameless, always-on-top window that appears in the top-left
//! corner of the screen for 3 seconds after a clip is successfully saved.

use eframe::egui;
use std::time::Instant;

/// Spawn the "Clip Saved" popup overlay via the persistent GUI manager.
///
/// Only one overlay is shown at a time. If one is already visible, the call
/// is a no-op.
pub fn run_clip_saved_overlay(filename: Option<String>) {
    crate::gui::manager::send_gui_message(crate::gui::manager::GuiMessage::ShowOverlay(filename));
}

pub fn render_overlay_direct(ctx: &egui::Context, filename: &Option<String>, shown_at: Instant, close_after_secs: f32) {
    let elapsed = shown_at.elapsed().as_secs_f32();

    if elapsed >= close_after_secs {
        // The manager will stop calling this, but we can also request close just in case
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        return;
    }

    // Fade-out in the last 0.8 seconds
    let alpha = if elapsed > close_after_secs - 0.8 {
        let fade_progress = (close_after_secs - elapsed) / 0.8;
        fade_progress.clamp(0.0, 1.0)
    } else {
        1.0
    };

    // Request continuous repainting for smooth fade
    ctx.request_repaint();

    // Dark semi-transparent panel — styled by us, not the OS
    let fill_color = egui::Color32::from_rgba_premultiplied(20, 20, 20, (220.0 * alpha) as u8);
    let stroke_color = egui::Color32::from_rgba_premultiplied(80, 180, 80, (120.0 * alpha) as u8);
    let frame = egui::Frame::default()
        .fill(fill_color)
        .corner_radius(egui::CornerRadius::same(10))
        .inner_margin(egui::Margin::symmetric(16, 12))
        .stroke(egui::Stroke::new(1.0, stroke_color));

    egui::CentralPanel::default()
        .frame(egui::Frame::NONE)
        .show(ctx, |ui| {
            frame.show(ui, |ui| {
                ui.horizontal(|ui| {
                    // Green check icon
                    ui.label(
                        egui::RichText::new("✓")
                            .size(22.0)
                            .color(egui::Color32::from_rgba_premultiplied(
                                80,
                                200,
                                80,
                                (255.0 * alpha) as u8,
                            )),
                    );

                    ui.add_space(6.0);

                    ui.vertical(|ui| {
                        ui.label(
                            egui::RichText::new("Clip Saved")
                                .size(14.0)
                                .strong()
                                .color(egui::Color32::from_rgba_premultiplied(
                                    240,
                                    240,
                                    240,
                                    (255.0 * alpha) as u8,
                                )),
                        );
                        if let Some(name) = filename {
                            ui.label(
                                egui::RichText::new(name)
                                    .size(11.0)
                                    .color(egui::Color32::from_rgba_premultiplied(
                                        160,
                                        160,
                                        160,
                                        (255.0 * alpha) as u8,
                                    )),
                            );
                        }
                    });
                });
            });
        });
}
