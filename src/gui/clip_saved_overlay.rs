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

    let visibility = overlay_visibility(elapsed, close_after_secs);
    let alpha = ease_out_cubic(visibility);
    let slide_y = (1.0 - alpha) * 10.0;
    let remaining = ((close_after_secs - elapsed) / close_after_secs.max(0.001)).clamp(0.0, 1.0);
    let title_color = with_alpha(egui::Color32::from_rgb(250, 252, 255), alpha);
    let subtitle_color = with_alpha(egui::Color32::from_rgb(176, 186, 201), alpha * 0.97);
    let accent_color = with_alpha(egui::Color32::from_rgb(88, 214, 141), alpha);
    let accent_soft = with_alpha(egui::Color32::from_rgb(88, 214, 141), alpha * 0.20);
    let shadow_color = with_alpha(egui::Color32::from_rgb(0, 0, 0), alpha * 0.28);
    let border_color = with_alpha(egui::Color32::from_rgb(58, 67, 82), alpha * 0.92);
    let panel_color = with_alpha(egui::Color32::from_rgb(17, 21, 28), alpha * 0.98);
    let subtitle = filename
        .as_deref()
        .map(|name| truncate_middle(name, 26))
        .unwrap_or_else(|| "Your clip is ready".to_owned());

    ctx.request_repaint();

    egui::CentralPanel::default()
        .frame(
            egui::Frame::default()
                .fill(egui::Color32::TRANSPARENT)
                .inner_margin(egui::Margin::symmetric(0, 0)),
        )
        .show(ctx, |ui| {
            let bounds = ui.max_rect();
            let painter = ui.painter_at(bounds);
            let card_size = egui::vec2(
                (bounds.width() - 12.0).clamp(164.0, 260.0),
                (bounds.height() - 8.0).clamp(58.0, 100.0),
            );
            let card_rect = egui::Rect::from_center_size(
                bounds.center() + egui::vec2(0.0, slide_y),
                card_size,
            );
            let shadow_rect = card_rect.translate(egui::vec2(0.0, 3.0));
            let inner_rect = card_rect.shrink(1.0);
            let stripe_rect = egui::Rect::from_min_max(
                inner_rect.min,
                egui::pos2(inner_rect.min.x + 4.0, inner_rect.max.y),
            );
            let icon_center = egui::pos2(inner_rect.min.x + 24.0, inner_rect.center().y + 0.5);
            let icon_radius = 11.0;
            let title_pos = egui::pos2(inner_rect.min.x + 42.0, inner_rect.min.y + 14.0);
            let subtitle_pos = egui::pos2(inner_rect.min.x + 42.0, inner_rect.min.y + 33.0);
            let progress_rect = egui::Rect::from_min_max(
                egui::pos2(inner_rect.min.x + 42.0, inner_rect.max.y - 7.0),
                egui::pos2(inner_rect.max.x - 10.0, inner_rect.max.y - 4.0),
            );
            let progress_fill = egui::Rect::from_min_max(
                progress_rect.min,
                egui::pos2(
                    progress_rect.min.x + progress_rect.width() * remaining,
                    progress_rect.max.y,
                ),
            );

            painter.rect_filled(shadow_rect, egui::CornerRadius::same(16), shadow_color);
            painter.rect_filled(card_rect, egui::CornerRadius::same(16), border_color);
            painter.rect_filled(inner_rect, egui::CornerRadius::same(15), panel_color);
            painter.rect_filled(stripe_rect, egui::CornerRadius::same(15), accent_soft);
            painter.circle_filled(icon_center, icon_radius, accent_soft);
            painter.circle_filled(icon_center, icon_radius - 4.0, accent_color);
            painter.text(
                icon_center,
                egui::Align2::CENTER_CENTER,
                "✓",
                egui::FontId::proportional(15.0),
                with_alpha(egui::Color32::from_rgb(10, 18, 12), alpha),
            );
            painter.text(
                title_pos,
                egui::Align2::LEFT_TOP,
                "Clip Saved",
                egui::FontId::proportional(17.0),
                title_color,
            );
            painter.text(
                subtitle_pos,
                egui::Align2::LEFT_TOP,
                subtitle,
                egui::FontId::proportional(11.0),
                subtitle_color,
            );
            painter.rect_filled(
                progress_rect,
                egui::CornerRadius::same(3),
                with_alpha(egui::Color32::from_rgb(44, 50, 61), alpha * 0.85),
            );
            painter.rect_filled(progress_fill, egui::CornerRadius::same(3), accent_color);
        });
}

fn overlay_visibility(elapsed: f32, close_after_secs: f32) -> f32 {
    let fade_in = (elapsed / 0.18).clamp(0.0, 1.0);
    let fade_out = ((close_after_secs - elapsed) / 0.45).clamp(0.0, 1.0);
    fade_in.min(fade_out)
}

fn ease_out_cubic(value: f32) -> f32 {
    let inverse = 1.0 - value.clamp(0.0, 1.0);
    1.0 - inverse * inverse * inverse
}

fn with_alpha(color: egui::Color32, alpha: f32) -> egui::Color32 {
    egui::Color32::from_rgba_premultiplied(
        color.r(),
        color.g(),
        color.b(),
        (255.0 * alpha.clamp(0.0, 1.0)).round() as u8,
    )
}

fn truncate_middle(text: &str, max_chars: usize) -> String {
    let char_count = text.chars().count();
    if char_count <= max_chars {
        return text.to_owned();
    }

    if max_chars <= 1 {
        return "…".to_owned();
    }

    let front = (max_chars - 1) / 2;
    let back = max_chars - 1 - front;
    let start: String = text.chars().take(front).collect();
    let end: String = text
        .chars()
        .rev()
        .take(back)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();

    format!("{start}…{end}")
}
