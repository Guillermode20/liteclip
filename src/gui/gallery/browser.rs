use eframe::egui;

use super::{
    format_compact_duration, format_size_mb, utils, BrowserUiOutcome, ClipCompressApp, VideoEntry,
    ALL_GAMES_FILTER,
};
use crate::gui::manager::{show_toast, ToastKind};

pub(super) fn render_browser_ui(app: &mut ClipCompressApp, ui: &mut egui::Ui) -> BrowserUiOutcome {
    let mut outcome = BrowserUiOutcome::default();
    let total_videos: usize = app
        .videos_by_game
        .iter()
        .map(|(_, videos)| videos.len())
        .sum();
    let display_groups: Vec<(String, Vec<VideoEntry>)> = if app.filter_game == ALL_GAMES_FILTER {
        app.videos_by_game.clone()
    } else {
        app.videos_by_game
            .iter()
            .find(|(game, _)| *game == app.filter_game)
            .map(|(game, videos)| vec![(game.clone(), videos.clone())])
            .unwrap_or_default()
    };

    let mut filter_response = None;
    ui.horizontal(|ui| {
        ui.heading("Clip & Compress");
        ui.label(format!("({total_videos} videos)"));
        if ui.button("Open Video File...").clicked() {
            outcome.request_import_video_dialog = true;
        }
        if ui.button("Open Folder").clicked() {
            if let Err(err) = utils::open_path_impl(&app.save_directory) {
                show_toast(ToastKind::Error, format!("Failed to open folder: {err:#}"));
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Refresh").clicked() {
                outcome.refresh_requested = true;
            }
            if ui
                .toggle_value(&mut app.selection_mode, "Select...")
                .clicked()
            {
                if !app.selection_mode {
                    app.selected_videos.clear();
                    app.delete_slider_progress = 0.0;
                    app.delete_hold_started_at = None;
                }
            }
            let response = egui::ComboBox::from_id_salt("clip_filter_game")
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
            filter_response = Some(response.response);
        });
    });

    if app.focus_filter_requested {
        if let Some(response) = filter_response {
            response.request_focus();
        }
        app.focus_filter_requested = false;
    }

    ui.separator();

    if let Some(error) = &app.scan_error {
        ui.colored_label(egui::Color32::LIGHT_RED, error);
    }

    if display_groups.is_empty() {
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
    let columns_count = ((ui.available_width() + tile_spacing) / (tile_width + tile_spacing))
        .floor()
        .max(1.0) as usize;

    if app.selection_mode && !app.selected_videos.is_empty() {
        egui::TopBottomPanel::bottom("delete_panel")
            .exact_height(50.0)
            .show_inside(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.add_space(10.0);
                    ui.label(
                        egui::RichText::new(format!("{} selected", app.selected_videos.len()))
                            .strong(),
                    );

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_space(20.0);

                        let response = ui.add(
                            egui::Slider::new(&mut app.delete_slider_progress, 0.0..=1.0)
                                .show_value(false)
                                .text("Slide to Confirm Delete"),
                        );

                        if response.drag_stopped() {
                            if app.delete_slider_progress >= 0.99 {
                                // Gather all matching video entries
                                let mut deleting = Vec::new();
                                for (_, videos) in &app.videos_by_game {
                                    for video in videos {
                                        if app.selected_videos.contains(&video.path) {
                                            deleting.push(video.clone());
                                        }
                                    }
                                }
                                outcome.videos_to_delete = deleting;
                                app.delete_hold_started_at = None;
                                app.delete_slider_progress = 0.0;
                            } else {
                                app.delete_slider_progress = 0.0;
                            }
                        } else if !response.dragged() && app.delete_hold_started_at.is_none() {
                            app.delete_slider_progress = 0.0;
                        }
                    });
                });
            });
    }

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for (game, videos) in display_groups {
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.heading(game.clone());
                    if app.selection_mode {
                        let game_video_paths: Vec<_> =
                            videos.iter().map(|v| v.path.clone()).collect();
                        let all_selected = game_video_paths
                            .iter()
                            .all(|p| app.selected_videos.contains(p));

                        if ui
                            .button(if all_selected {
                                "Deselect All"
                            } else {
                                "Select All"
                            })
                            .clicked()
                        {
                            if all_selected {
                                for path in game_video_paths {
                                    app.selected_videos.remove(&path);
                                }
                            } else {
                                for path in game_video_paths {
                                    app.selected_videos.insert(path);
                                }
                            }
                        }
                    }
                });
                ui.add_space(4.0);

                let group_rows = videos.len().div_ceil(columns_count);
                for row in 0..group_rows {
                    ui.horizontal(|ui| {
                        for column in 0..columns_count {
                            let index = row * columns_count + column;
                            if index >= videos.len() {
                                break;
                            }

                            let video = videos[index].clone();
                            let has_thumb = app.thumbnails.contains_key(&video.path);
                            if !has_thumb {
                                outcome.thumbnails_to_generate.push(video.path.clone());
                            }

                            let response = ui
                                .scope(|ui| {
                                    ui.set_min_width(tile_width);
                                    ui.set_max_width(tile_width);
                                    let frame = egui::Frame::group(ui.style())
                                        .fill(egui::Color32::from_rgb(30, 32, 36))
                                        .inner_margin(egui::Margin::same(10));

                                    frame.show(ui, |ui| {
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

                                            if !app.selection_mode {
                                                ui.add_space(6.0);
                                                ui.horizontal(|ui| {
                                                    if ui.button("Edit").clicked() {
                                                        outcome.selected_video =
                                                            Some(video.clone());
                                                    }
                                                    if ui.button("Open").clicked() {
                                                        outcome.video_to_open = Some(video.clone());
                                                    }
                                                    ui.with_layout(
                                                        egui::Layout::right_to_left(
                                                            egui::Align::Center,
                                                        ),
                                                        |ui| {
                                                            if ui.button("Delete").clicked() {
                                                                app.selection_mode = true;
                                                                app.selected_videos
                                                                    .insert(video.path.clone());
                                                            }
                                                        },
                                                    );
                                                });
                                            } else {
                                                // In selection mode, adding a bit of space so height matches mostly
                                                ui.add_space(32.0);
                                            }
                                        });
                                    })
                                })
                                .response;

                            let is_selected = app.selected_videos.contains(&video.path);

                            // Draw selection/focus outlines when in delete-selection mode.
                            if app.selection_mode && is_selected {
                                let rect = response.rect.shrink(2.0);
                                ui.painter().rect_stroke(
                                    rect,
                                    // Shrink the radius to match the inner edge of the tile frame
                                    egui::CornerRadius::same(4),
                                    egui::Stroke::new(2.0, egui::Color32::WHITE),
                                    egui::StrokeKind::Inside,
                                );
                            }

                            if app.selection_mode {
                                // Make the whole tile clickable
                                let interact_response = ui.interact(
                                    response.rect,
                                    response.id.with("click"),
                                    egui::Sense::click(),
                                );
                                if interact_response.clicked() {
                                    if app.selected_videos.contains(&video.path) {
                                        app.selected_videos.remove(&video.path);
                                    } else {
                                        app.selected_videos.insert(video.path.clone());
                                    }
                                }
                            } else if response.double_clicked() {
                                outcome.selected_video = Some(video);
                            }
                            ui.add_space(tile_spacing);
                        }
                    });
                    ui.add_space(tile_spacing);
                }
            }
        });

    outcome
}
