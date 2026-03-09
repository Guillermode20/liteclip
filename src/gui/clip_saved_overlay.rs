//! Clip-saved popup overlay

use eframe::egui;
use std::time::Instant;

pub fn run_clip_saved_overlay(filename: Option<String>) {
    crate::gui::manager::send_gui_message(crate::gui::manager::GuiMessage::ShowOverlay(filename));
}

pub fn render_overlay_direct(
    ctx: &egui::Context,
    filename: &Option<String>,
    shown_at: Instant,
    close_after_secs: f32,
) {
    let elapsed = shown_at.elapsed().as_secs_f32();

    if elapsed >= close_after_secs {
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        return;
    }

    let alpha = if elapsed > close_after_secs - 0.5 {
        (close_after_secs - elapsed) / 0.5
    } else {
        1.0
    };

    ctx.request_repaint();

    egui::CentralPanel::default()
        .frame(egui::Frame::NONE)
        .show(ctx, |ui| {
            let frame = egui::Frame::default()
                .fill(egui::Color32::from_rgba_premultiplied(
                    30,
                    30,
                    35,
                    (220.0 * alpha) as u8,
                ))
                .corner_radius(egui::CornerRadius::same(6))
                .inner_margin(egui::Margin::symmetric(10, 6))
                .stroke(egui::Stroke::new(
                    1.0,
                    egui::Color32::from_rgba_premultiplied(70, 180, 70, (180.0 * alpha) as u8),
                ));

            frame.show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.label(egui::RichText::new("✓").size(14.0).color(
                        egui::Color32::from_rgba_premultiplied(70, 200, 70, (255.0 * alpha) as u8),
                    ));

                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new("Clip Saved").size(11.0).strong().color(
                            egui::Color32::from_rgba_premultiplied(
                                230,
                                230,
                                230,
                                (255.0 * alpha) as u8,
                            ),
                        ));

                        if let Some(name) = filename {
                            let display = if name.len() > 18 { &name[..15] } else { name };
                            ui.label(egui::RichText::new(display).size(9.0).color(
                                egui::Color32::from_rgba_premultiplied(
                                    140,
                                    140,
                                    140,
                                    (200.0 * alpha) as u8,
                                ),
                            ));
                        }
                    });
                });
            });
        });
}
