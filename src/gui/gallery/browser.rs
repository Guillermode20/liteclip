use eframe::egui;

use super::{
    format_compact_duration, format_size_mb, BrowserUiOutcome, ClipCompressApp, VideoEntry,
    ALL_GAMES_FILTER,
};

pub(super) fn render_browser_ui(app: &mut ClipCompressApp, ui: &mut egui::Ui) -> BrowserUiOutcome {
    let mut outcome = BrowserUiOutcome::default();
    let total_videos: usize = app
        .videos_by_game
        .iter()
        .map(|(_, videos)| videos.len())
        .sum();
    let filtered_videos: Vec<VideoEntry> = if app.filter_game == ALL_GAMES_FILTER {
        app.videos_by_game
            .iter()
            .flat_map(|(_, videos)| videos.iter().cloned())
            .collect()
    } else {
        app.videos_by_game
            .iter()
            .find(|(game, _)| *game == app.filter_game)
            .map(|(_, videos)| videos.clone())
            .unwrap_or_default()
    };

    ui.horizontal(|ui| {
        ui.heading("Clip & Compress");
        ui.label(format!("({total_videos} videos)"));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Refresh").clicked() {
                outcome.refresh_requested = true;
            }
            egui::ComboBox::from_id_salt("clip_filter_game")
                .selected_text(&app.filter_game)
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut app.filter_game,
                        ALL_GAMES_FILTER.to_string(),
                        ALL_GAMES_FILTER,
                    );
                    for (game, _) in &app.videos_by_game {
                        ui.selectable_value(&mut app.filter_game, game.clone(), game);
                    }
                });
        });
    });
    ui.separator();

    if let Some(error) = &app.scan_error {
        ui.colored_label(egui::Color32::LIGHT_RED, error);
    }

    if filtered_videos.is_empty() {
        ui.vertical_centered(|ui| {
            ui.add_space(64.0);
            ui.label(
                egui::RichText::new("No saved videos found")
                    .size(18.0)
                    .strong(),
            );
            ui.label(
                egui::RichText::new("Save some clips first, then open Clip & Compress again.")
                    .weak(),
            );
        });
        return outcome;
    }

    let tile_width = 220.0;
    let tile_spacing = 12.0;
    let thumb_height = 124.0;
    let columns = ((ui.available_width() + tile_spacing) / (tile_width + tile_spacing))
        .floor()
        .max(1.0) as usize;
    let rows = filtered_videos.len().div_ceil(columns);

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for row in 0..rows {
                ui.horizontal(|ui| {
                    for column in 0..columns {
                        let index = row * columns + column;
                        if index >= filtered_videos.len() {
                            break;
                        }

                        let video = filtered_videos[index].clone();
                        let has_thumb = app.thumbnails.contains_key(&video.path);
                        if !has_thumb {
                            outcome.thumbnails_to_generate.push(video.clone());
                        }

                        let response = ui
                            .scope(|ui| {
                                ui.set_min_width(tile_width);
                                ui.set_max_width(tile_width);
                                egui::Frame::group(ui.style())
                                    .fill(egui::Color32::from_rgb(30, 32, 36))
                                    .inner_margin(egui::Margin::same(10))
                                    .show(ui, |ui| {
                                        ui.vertical(|ui| {
                                            let thumb_size =
                                                egui::vec2(tile_width - 20.0, thumb_height);
                                            if let Some(texture) = app.thumbnails.get(&video.path) {
                                                ui.add(
                                                    egui::Image::from_texture(texture)
                                                        .fit_to_exact_size(thumb_size)
                                                        .maintain_aspect_ratio(true),
                                                );
                                            } else {
                                                let (rect, _) = ui.allocate_exact_size(
                                                    thumb_size,
                                                    egui::Sense::hover(),
                                                );
                                                ui.painter().rect_filled(
                                                    rect,
                                                    egui::CornerRadius::same(6),
                                                    egui::Color32::from_rgb(22, 24, 27),
                                                );
                                                ui.painter().text(
                                                    rect.center(),
                                                    egui::Align2::CENTER_CENTER,
                                                    format!(
                                                        "{}\n{}",
                                                        format_compact_duration(
                                                            video.metadata.duration_secs
                                                        ),
                                                        "Generating thumbnail"
                                                    ),
                                                    egui::FontId::proportional(14.0),
                                                    egui::Color32::from_rgb(150, 155, 165),
                                                );
                                            }

                                            ui.add_space(6.0);
                                            ui.label(
                                                egui::RichText::new(&video.filename)
                                                    .size(13.0)
                                                    .strong(),
                                            );
                                            ui.label(
                                                egui::RichText::new(format!(
                                                    "{} | {} | {}",
                                                    video.game,
                                                    format_compact_duration(
                                                        video.metadata.duration_secs
                                                    ),
                                                    format_size_mb(video.size_mb)
                                                ))
                                                .weak(),
                                            );
                                            ui.label(
                                                egui::RichText::new(format!(
                                                    "{}x{}{}",
                                                    video.metadata.width,
                                                    video.metadata.height,
                                                    if video.metadata.has_audio {
                                                        " | audio"
                                                    } else {
                                                        ""
                                                    }
                                                ))
                                                .small(),
                                            );
                                            ui.add_space(6.0);
                                            if ui.button("Select").clicked() {
                                                outcome.selected_video = Some(video.clone());
                                            }
                                        });
                                    })
                            })
                            .response;

                        if response.double_clicked() {
                            outcome.selected_video = Some(video);
                        }
                        ui.add_space(tile_spacing);
                    }
                });
                ui.add_space(tile_spacing);
            }
        });

    outcome
}
