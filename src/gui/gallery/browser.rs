use eframe::egui;
use std::time::{Duration, SystemTime};

use super::{
    format_compact_duration, format_size_mb, utils, BrowserUiOutcome, CardSize, ClipCompressApp,
    DateFilter, DurationFilter, SizeFilter, SortBy, VideoEntry, ALL_GAMES_FILTER,
};
use crate::gui::manager::{show_toast, ToastKind};

pub(super) struct FilteredGroup {
    pub(super) game: String,
    pub(super) videos: Vec<VideoEntry>,
}

pub(super) struct FilteredCache {
    pub(super) groups: Vec<FilteredGroup>,
    pub(super) total_count: usize,
    search_query: String,
    filter_game: String,
    date_filter: DateFilter,
    duration_filter: DurationFilter,
    size_filter: SizeFilter,
    sort_by: SortBy,
    show_clipped_only: bool,
}

impl FilteredCache {
    pub(super) fn matches(&self, app: &ClipCompressApp) -> bool {
        self.search_query == app.search_query
            && self.filter_game == app.filter_game
            && self.date_filter == app.date_filter
            && self.duration_filter == app.duration_filter
            && self.size_filter == app.size_filter
            && self.sort_by == app.sort_by
            && self.show_clipped_only == app.show_clipped_only
    }
}

pub(super) fn ensure_filter_cache(app: &mut ClipCompressApp) {
    let needs_recompute = match &app.filtered_cache {
        None => true,
        Some(cache) => !cache.matches(app),
    };
    if needs_recompute {
        app.filtered_cache = Some(compute_filtered_cache(app));
    }
}

fn compute_filtered_cache(app: &ClipCompressApp) -> FilteredCache {
    let now = SystemTime::now();
    let query_lower = app.search_query.to_lowercase();

    let groups: Vec<FilteredGroup> = app
        .videos_by_game
        .iter()
        .filter_map(|(game, videos)| {
            if app.filter_game != ALL_GAMES_FILTER && *game != app.filter_game {
                return None;
            }

            let filtered: Vec<VideoEntry> = videos
                .iter()
                .filter(|video| {
                    if !app.search_query.is_empty()
                        && !video.filename_lower.contains(&query_lower)
                        && !video.game_lower.contains(&query_lower)
                    {
                        return false;
                    }

                    match app.date_filter {
                        DateFilter::Last24Hours => {
                            let day_ago = now - Duration::from_secs(24 * 60 * 60);
                            if video.modified < day_ago {
                                return false;
                            }
                        }
                        DateFilter::Last7Days => {
                            let week_ago = now - Duration::from_secs(7 * 24 * 60 * 60);
                            if video.modified < week_ago {
                                return false;
                            }
                        }
                        DateFilter::Last30Days => {
                            let month_ago = now - Duration::from_secs(30 * 24 * 60 * 60);
                            if video.modified < month_ago {
                                return false;
                            }
                        }
                        DateFilter::AllTime => {}
                    }

                    match app.duration_filter {
                        DurationFilter::Short => {
                            if video.metadata.duration_secs >= 30.0 {
                                return false;
                            }
                        }
                        DurationFilter::Medium => {
                            if !(30.0..=300.0).contains(&video.metadata.duration_secs) {
                                return false;
                            }
                        }
                        DurationFilter::Long => {
                            if video.metadata.duration_secs <= 300.0 {
                                return false;
                            }
                        }
                        DurationFilter::All => {}
                    }

                    match app.size_filter {
                        SizeFilter::Small => {
                            if video.size_mb >= 10.0 {
                                return false;
                            }
                        }
                        SizeFilter::Medium => {
                            if video.size_mb < 10.0 || video.size_mb > 50.0 {
                                return false;
                            }
                        }
                        SizeFilter::Large => {
                            if video.size_mb <= 50.0 {
                                return false;
                            }
                        }
                        SizeFilter::All => {}
                    }

                    if app.show_clipped_only && !video.is_clipped {
                        return false;
                    }

                    true
                })
                .cloned()
                .collect();

            let mut sorted = filtered;
            match app.sort_by {
                SortBy::DateNewest => {
                    sorted.sort_by(|a, b| b.modified.cmp(&a.modified));
                }
                SortBy::DateOldest => {
                    sorted.sort_by(|a, b| a.modified.cmp(&b.modified));
                }
                SortBy::NameAZ => {
                    sorted.sort_by(|a, b| a.filename.cmp(&b.filename));
                }
                SortBy::NameZA => {
                    sorted.sort_by(|a, b| b.filename.cmp(&a.filename));
                }
                SortBy::SizeLarge => {
                    sorted.sort_by(|a, b| {
                        b.size_mb
                            .partial_cmp(&a.size_mb)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    });
                }
                SortBy::SizeSmall => {
                    sorted.sort_by(|a, b| {
                        a.size_mb
                            .partial_cmp(&b.size_mb)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    });
                }
                SortBy::DurationLong => {
                    sorted.sort_by(|a, b| {
                        b.metadata
                            .duration_secs
                            .partial_cmp(&a.metadata.duration_secs)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    });
                }
                SortBy::DurationShort => {
                    sorted.sort_by(|a, b| {
                        a.metadata
                            .duration_secs
                            .partial_cmp(&b.metadata.duration_secs)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    });
                }
            }

            if sorted.is_empty() {
                None
            } else {
                Some(FilteredGroup {
                    game: game.clone(),
                    videos: sorted,
                })
            }
        })
        .collect();

    let total_count: usize = groups.iter().map(|g| g.videos.len()).sum();

    FilteredCache {
        groups,
        total_count,
        search_query: app.search_query.clone(),
        filter_game: app.filter_game.clone(),
        date_filter: app.date_filter,
        duration_filter: app.duration_filter,
        size_filter: app.size_filter,
        sort_by: app.sort_by,
        show_clipped_only: app.show_clipped_only,
    }
}

pub(super) fn render_browser_ui(app: &mut ClipCompressApp, ui: &mut egui::Ui) -> BrowserUiOutcome {
    let mut outcome = BrowserUiOutcome::default();

    ensure_filter_cache(app);

    let filtered_count = app
        .filtered_cache
        .as_ref()
        .map_or(0, |cache| cache.total_count);

    ui.horizontal(|ui| {
        ui.heading("Clip & Compress");
        ui.label(format!("({} videos)", filtered_count));

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let size_btn_text = match app.card_size {
                CardSize::Small => "Small",
                CardSize::Medium => "Medium",
                CardSize::Large => "Large",
                CardSize::XLarge => "XL",
            };
            if ui.button(size_btn_text).clicked() {
                app.card_size = app.card_size.next();
            }
            if ui.button("Refresh").clicked() {
                outcome.refresh_requested = true;
            }
            if ui
                .toggle_value(&mut app.selection_mode, "Select...")
                .clicked()
                && !app.selection_mode
            {
                app.selected_videos.clear();
                app.delete_slider_progress = 0.0;
                app.delete_hold_started_at = None;
            }
            if ui.button("Open Folder").clicked() {
                if let Err(err) = utils::open_path_impl(&app.save_directory) {
                    show_toast(ToastKind::Error, format!("Failed to open folder: {err:#}"));
                }
            }
            if ui.button("Open Video File...").clicked() {
                outcome.request_import_video_dialog = true;
            }
        });
    });

    ui.separator();

    render_filter_bar(app, ui);

    ui.separator();

    if let Some(error) = &app.scan_error {
        ui.colored_label(egui::Color32::LIGHT_RED, error);
    }

    let cache = app
        .filtered_cache
        .take()
        .expect("filtered_cache should always be initialized before browser render");

    if !app.loaded {
        app.filtered_cache = Some(cache);
        ui.vertical_centered(|ui| {
            ui.add_space(64.0);
            ui.add(egui::Spinner::new().size(32.0));
            ui.add_space(8.0);
            ui.label(egui::RichText::new("Scanning for videos...").weak());
        });
        return outcome;
    }

    if cache.groups.is_empty() {
        app.filtered_cache = Some(cache);
        render_empty_state(ui, app.search_query.is_empty());
        return outcome;
    }

    let (tile_width, thumb_height) = app.card_size.dimensions();
    let tile_spacing = 12.0;
    let columns_count = ((ui.available_width() + tile_spacing) / (tile_width + tile_spacing))
        .floor()
        .max(1.0) as usize;

    if app.selection_mode && !app.selected_videos.is_empty() {
        render_delete_panel(app, ui, &mut outcome);
    }

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for group in &cache.groups {
                render_game_section(
                    app,
                    ui,
                    &group.game,
                    &group.videos,
                    tile_width,
                    thumb_height,
                    tile_spacing,
                    columns_count,
                    &mut outcome,
                );
            }
        });

    app.filtered_cache = Some(cache);
    outcome
}

fn render_filter_bar(app: &mut ClipCompressApp, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        let _search_response = ui.add(
            egui::TextEdit::singleline(&mut app.search_query)
                .hint_text("Search videos...")
                .desired_width(200.0),
        );

        ui.add_space(8.0);

        egui::ComboBox::from_id_salt("clip_filter_game")
            .selected_text(&app.filter_game)
            .width(140.0)
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

        ui.add_space(8.0);

        egui::ComboBox::from_id_salt("clip_filter_date")
            .selected_text(app.date_filter.label())
            .width(120.0)
            .show_ui(ui, |ui| {
                for filter in [
                    DateFilter::AllTime,
                    DateFilter::Last24Hours,
                    DateFilter::Last7Days,
                    DateFilter::Last30Days,
                ] {
                    ui.selectable_value(&mut app.date_filter, filter, filter.label());
                }
            });

        ui.add_space(8.0);

        egui::ComboBox::from_id_salt("clip_filter_duration")
            .selected_text(app.duration_filter.label())
            .width(130.0)
            .show_ui(ui, |ui| {
                for filter in [
                    DurationFilter::All,
                    DurationFilter::Short,
                    DurationFilter::Medium,
                    DurationFilter::Long,
                ] {
                    ui.selectable_value(&mut app.duration_filter, filter, filter.label());
                }
            });

        ui.add_space(8.0);

        egui::ComboBox::from_id_salt("clip_filter_size")
            .selected_text(app.size_filter.label())
            .width(130.0)
            .show_ui(ui, |ui| {
                for filter in [
                    SizeFilter::All,
                    SizeFilter::Small,
                    SizeFilter::Medium,
                    SizeFilter::Large,
                ] {
                    ui.selectable_value(&mut app.size_filter, filter, filter.label());
                }
            });

        ui.add_space(8.0);

        egui::ComboBox::from_id_salt("clip_sort_by")
            .selected_text(app.sort_by.label())
            .width(160.0)
            .show_ui(ui, |ui| {
                for sort in [
                    SortBy::DateNewest,
                    SortBy::DateOldest,
                    SortBy::NameAZ,
                    SortBy::NameZA,
                    SortBy::SizeLarge,
                    SortBy::SizeSmall,
                    SortBy::DurationLong,
                    SortBy::DurationShort,
                ] {
                    ui.selectable_value(&mut app.sort_by, sort, sort.label());
                }
            });

        ui.add_space(8.0);

        ui.checkbox(&mut app.show_clipped_only, "Clips Only");

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let has_filters = !app.search_query.is_empty()
                || app.filter_game != ALL_GAMES_FILTER
                || app.date_filter != DateFilter::AllTime
                || app.duration_filter != DurationFilter::All
                || app.size_filter != SizeFilter::All
                || app.show_clipped_only;

            if has_filters && ui.button("Clear Filters").clicked() {
                app.search_query.clear();
                app.filter_game = ALL_GAMES_FILTER.to_string();
                app.date_filter = DateFilter::AllTime;
                app.duration_filter = DurationFilter::All;
                app.size_filter = SizeFilter::All;
                app.show_clipped_only = false;
            }
        });
    });
}

fn render_empty_state(ui: &mut egui::Ui, no_search_results: bool) {
    ui.vertical_centered(|ui| {
        ui.add_space(64.0);
        if no_search_results {
            ui.label(
                egui::RichText::new("No saved videos found")
                    .size(18.0)
                    .strong(),
            );
            ui.label(
                egui::RichText::new("Save some clips first, then open Clip & Compress again.")
                    .weak(),
            );
        } else {
            ui.label(
                egui::RichText::new("No videos match your filters")
                    .size(18.0)
                    .strong(),
            );
            ui.label(egui::RichText::new("Try adjusting your search or filter criteria.").weak());
        }
    });
}

fn render_delete_panel(
    app: &mut ClipCompressApp,
    ui: &mut egui::Ui,
    outcome: &mut BrowserUiOutcome,
) {
    egui::TopBottomPanel::bottom("delete_panel")
        .exact_height(50.0)
        .show_inside(ui, |ui| {
            ui.horizontal(|ui| {
                ui.add_space(10.0);
                ui.label(
                    egui::RichText::new(format!("{} selected", app.selected_videos.len())).strong(),
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

#[allow(clippy::too_many_arguments)]
fn render_game_section(
    app: &mut ClipCompressApp,
    ui: &mut egui::Ui,
    game: &str,
    videos: &[VideoEntry],
    tile_width: f32,
    thumb_height: f32,
    tile_spacing: f32,
    columns_count: usize,
    outcome: &mut BrowserUiOutcome,
) {
    let is_collapsed = app.collapsed_games.contains(game);
    let video_count = videos.len();

    ui.add_space(8.0);

    ui.horizontal(|ui| {
        let collapse_text = if is_collapsed { "▶" } else { "▼" };
        if ui.button(collapse_text).clicked() {
            if is_collapsed {
                app.collapsed_games.remove(game);
            } else {
                app.collapsed_games.insert(game.to_string());
            }
        }

        ui.heading(format!("{} ({})", game, video_count));

        if app.selection_mode {
            let game_video_paths: Vec<_> = videos.iter().map(|v| v.path.clone()).collect();
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

    if !is_collapsed {
        ui.add_space(4.0);

        let group_rows = videos.len().div_ceil(columns_count);
        for row in 0..group_rows {
            ui.horizontal(|ui| {
                for column in 0..columns_count {
                    let index = row * columns_count + column;
                    if index >= videos.len() {
                        break;
                    }

                    render_video_card(
                        app,
                        ui,
                        &videos[index],
                        tile_width,
                        thumb_height,
                        tile_spacing,
                        outcome,
                    );
                }
            });
            ui.add_space(tile_spacing);
        }
    }

    ui.add_space(8.0);
}

fn render_video_card(
    app: &mut ClipCompressApp,
    ui: &mut egui::Ui,
    video: &VideoEntry,
    tile_width: f32,
    thumb_height: f32,
    tile_spacing: f32,
    outcome: &mut BrowserUiOutcome,
) {
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
                    let thumb_size = egui::vec2(tile_width - 20.0, thumb_height);
                    if let Some(texture) = app.thumbnails.get(&video.path) {
                        ui.add(
                            egui::Image::from_texture(texture)
                                .fit_to_exact_size(thumb_size)
                                .maintain_aspect_ratio(true),
                        );
                    } else {
                        let (rect, _) = ui.allocate_exact_size(thumb_size, egui::Sense::hover());
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
                                format_compact_duration(video.metadata.duration_secs),
                                "Generating thumbnail"
                            ),
                            egui::FontId::proportional(14.0),
                            egui::Color32::from_rgb(150, 155, 165),
                        );
                    }

                    ui.add_space(6.0);

                    let filename_text = if video.is_clipped {
                        format!("[C] {}", video.filename)
                    } else {
                        video.filename.clone()
                    };
                    ui.label(egui::RichText::new(&filename_text).size(13.0).strong());

                    ui.label(
                        egui::RichText::new(format!(
                            "{} | {}",
                            format_compact_duration(video.metadata.duration_secs),
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
                                outcome.selected_video = Some(video.clone());
                            }
                            if ui.button("Open").clicked() {
                                outcome.video_to_open = Some(video.clone());
                            }
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui.button("Delete").clicked() {
                                        app.selection_mode = true;
                                        app.selected_videos.insert(video.path.clone());
                                    }
                                },
                            );
                        });
                    } else {
                        ui.add_space(32.0);
                    }
                });
            })
        })
        .response;

    let is_selected = app.selected_videos.contains(&video.path);

    if app.selection_mode && is_selected {
        let rect = response.rect.shrink(2.0);
        ui.painter().rect_stroke(
            rect,
            egui::CornerRadius::same(4),
            egui::Stroke::new(2.0, egui::Color32::WHITE),
            egui::StrokeKind::Inside,
        );
    }

    if app.selection_mode {
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
        outcome.selected_video = Some(video.clone());
    }
    ui.add_space(tile_spacing);
}
