use eframe::egui;

use super::{
    add_cut_point, estimate_export_bitrates_from_editor, format_compact_duration, format_size_mb,
    format_timestamp_precise, remove_cut_point, start_export, toggle_editor_playback, x_to_time,
    EditorState, EditorUiOutcome, DEFAULT_TARGET_SIZE_MB, EDITOR_SIDEBAR_MIN_WIDTH,
    EDITOR_SIDEBAR_WIDTH, EDITOR_STACK_BREAKPOINT, SCRUB_FAST_RATE_SECS_PER_SEC,
    SCRUB_SAMPLE_MIN_DT_SECS,
};

pub(super) fn render_preview_panel_impl(
    ui: &mut egui::Ui,
    editor: &mut EditorState,
    outcome: &mut EditorUiOutcome,
) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        let available_width = ui.available_width().max(220.0);
        let wide_layout = available_width >= 1200.0;
        let aspect_ratio = (editor.video.metadata.width.max(1) as f32
            / editor.video.metadata.height.max(1) as f32)
            .max(1.0 / 3.0);
        let available_height = ui.available_height().max(220.0);
        let reserved_height = (available_height * 0.14).clamp(64.0, 140.0);
        let max_preview_height = (available_height - reserved_height).clamp(200.0, 860.0);
        let max_preview_width = if wide_layout {
            (max_preview_height * aspect_ratio).min(available_width)
        } else {
            available_width
        };
        let mut preview_height = (max_preview_width / aspect_ratio).max(200.0);
        preview_height = preview_height.min(max_preview_height);
        let preview_size = egui::vec2(available_width, preview_height);

        if let Some(texture) = &editor.preview_texture {
            ui.add(
                egui::Image::from_texture(texture)
                    .fit_to_exact_size(preview_size)
                    .maintain_aspect_ratio(true),
            );
        } else {
            let (rect, _) = ui.allocate_exact_size(preview_size, egui::Sense::hover());
            ui.painter().rect_filled(
                rect,
                egui::CornerRadius::same(6),
                egui::Color32::from_rgb(18, 20, 24),
            );
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Loading embedded preview...",
                egui::FontId::proportional(18.0),
                egui::Color32::from_rgb(160, 165, 175),
            );
            if !editor.preview_frame_in_flight {
                outcome.preview_request = Some(editor.current_time_secs);
            }
        }

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let label = if editor.is_playing { "Pause" } else { "Play" };
            if ui.button(label).clicked() {
                toggle_editor_playback(editor);
            }
            ui.label(format!(
                "Current: {}",
                format_timestamp_precise(editor.current_time_secs)
            ));
        });

        let duration = editor.duration_secs();
        let response = ui.add(
            egui::Slider::new(&mut editor.current_time_secs, 0.0..=duration).show_value(false),
        );
        if response.changed() {
            editor.playback.pause_at(editor.current_time_secs);
            editor.is_playing = false;
            editor.current_time_secs = editor.current_time_secs.clamp(0.0, editor.duration_secs());
            outcome.preview_request = Some(editor.current_time_secs);
        }
    });
}

fn render_editor_stats(ui: &mut egui::Ui, editor: &EditorState) {
    ui.horizontal_wrapped(|ui| {
        ui.label(format!(
            "Duration: {}",
            format_compact_duration(editor.duration_secs())
        ));
        ui.separator();
        ui.label(format!(
            "Original Size: {}",
            format_size_mb(editor.video.size_mb)
        ));
        ui.separator();
        ui.label(format!(
            "Output Duration: {}",
            format_compact_duration(editor.kept_duration_secs())
        ));
        ui.separator();
        ui.label(format!(
            "Resolution: {}x{}",
            editor.video.metadata.width, editor.video.metadata.height
        ));
        if editor.video.metadata.has_audio {
            ui.separator();
            ui.label("Audio: included");
        }
        ui.separator();
        let (cache_entries, cache_mb) = editor.playback.cache_stats();
        ui.label(format!(
            "Cache: {} frames ({:.1} MB)",
            cache_entries, cache_mb
        ));
    });
}

pub(super) fn clamp_selected_snippet_index_impl(editor: &mut EditorState) {
    let snippet_count = editor.snippets().len();
    if snippet_count == 0 {
        editor.selected_snippet_index = None;
        return;
    }

    let current = editor.selected_snippet_index.unwrap_or(0);
    editor.selected_snippet_index = Some(current.min(snippet_count - 1));
}

fn render_timeline_panel(
    ui: &mut egui::Ui,
    editor: &mut EditorState,
    outcome: &mut EditorUiOutcome,
) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.label(egui::RichText::new("Timeline").strong());
        ui.add_space(4.0);

        ui.horizontal(|ui| {
            if ui.button("Add Cut at Playhead (A)").clicked()
                && add_cut_point(editor, editor.current_time_secs)
            {
                outcome.preview_request = Some(editor.current_time_secs);
            }
            if ui
                .add_enabled(
                    editor.selected_cut_point.is_some(),
                    egui::Button::new("Remove Selected Cut (Del)"),
                )
                .clicked()
            {
                if let Some(index) = editor.selected_cut_point {
                    remove_cut_point(editor, index);
                    outcome.preview_request = Some(editor.current_time_secs);
                }
            }
            if ui.button("Clear All Splits").clicked() {
                super::clear_cut_points(editor);
                outcome.preview_request = Some(editor.current_time_secs);
            }
        });

        ui.add_space(8.0);

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .max_height(ui.available_height().clamp(120.0, 280.0))
            .show(ui, |ui| {
                render_timeline(ui, editor, outcome);
            });
    });
}

fn render_timeline(ui: &mut egui::Ui, editor: &mut EditorState, outcome: &mut EditorUiOutcome) {
    let timeline_height = ui.available_height().clamp(96.0, 190.0);
    let desired_size = egui::vec2(ui.available_width(), timeline_height);
    let (rect, response) = ui.allocate_exact_size(desired_size, egui::Sense::click_and_drag());
    let track_rect = egui::Rect::from_min_max(
        rect.min + egui::vec2(0.0, 12.0),
        rect.max - egui::vec2(0.0, 22.0),
    );
    let painter = ui.painter();
    for snippet in editor.snippets() {
        let left = super::time_to_x(track_rect, snippet.start_secs, editor.duration_secs());
        let right =
            super::time_to_x(track_rect, snippet.end_secs, editor.duration_secs()).max(left + 2.0);
        let snippet_rect = egui::Rect::from_min_max(
            egui::pos2(left, track_rect.top()),
            egui::pos2(right, track_rect.bottom()),
        );
        let color = if snippet.enabled {
            egui::Color32::from_rgb(48, 92, 156)
        } else {
            egui::Color32::from_rgb(96, 44, 44)
        };
        painter.rect_filled(snippet_rect, egui::CornerRadius::same(6), color);
    }

    for (index, cut_point) in editor.cut_points.iter().enumerate() {
        let x = super::time_to_x(track_rect, *cut_point, editor.duration_secs());
        let color = if editor.selected_cut_point == Some(index) {
            egui::Color32::WHITE
        } else {
            egui::Color32::from_rgb(220, 220, 220)
        };
        painter.line_segment(
            [
                egui::pos2(x, track_rect.top()),
                egui::pos2(x, track_rect.bottom()),
            ],
            egui::Stroke::new(2.0, color),
        );
    }

    let playhead_x = super::time_to_x(track_rect, editor.current_time_secs, editor.duration_secs());
    painter.line_segment(
        [
            egui::pos2(playhead_x, track_rect.top() - 4.0),
            egui::pos2(playhead_x, track_rect.bottom() + 4.0),
        ],
        egui::Stroke::new(2.0, egui::Color32::from_rgb(236, 201, 75)),
    );

    for ratio in [0.0_f32, 0.25, 0.5, 0.75, 1.0] {
        let time = editor.duration_secs() * f64::from(ratio);
        let x = egui::lerp(track_rect.left()..=track_rect.right(), ratio);
        painter.text(
            egui::pos2(x, rect.bottom() - 2.0),
            egui::Align2::CENTER_BOTTOM,
            format_compact_duration(time),
            egui::FontId::proportional(11.0),
            egui::Color32::from_rgb(160, 165, 175),
        );
    }

    if response.drag_started() {
        editor.was_playing_before_scrub = editor.is_playing;
        if editor.is_playing {
            editor.is_playing = false;
            editor.playback.pause_at(editor.current_time_secs);
        }
    }

    if let Some(pointer) = response.interact_pointer_pos() {
        if response.clicked() || response.dragged() {
            if let Some(index) = hit_test_cut_point(editor, track_rect, pointer.x) {
                editor.selected_cut_point = Some(index);
            } else {
                editor.selected_cut_point = None;
                let new_time_secs = x_to_time(track_rect, pointer.x, editor.duration_secs());

                let now = std::time::Instant::now();
                let mut is_fast_scrub = false;
                if let (Some(last_time), Some(last_pos)) =
                    (editor.last_scrub_time, editor.last_scrub_position)
                {
                    let dt = last_time.elapsed().as_secs_f64();
                    let dx = (new_time_secs - last_pos).abs();
                    if dt >= SCRUB_SAMPLE_MIN_DT_SECS {
                        let speed = dx / dt;
                        is_fast_scrub = speed >= SCRUB_FAST_RATE_SECS_PER_SEC;
                    }
                }
                editor.last_scrub_time = Some(now);
                editor.last_scrub_position = Some(new_time_secs);
                editor.current_time_secs = new_time_secs;

                if response.clicked() {
                    if editor.is_playing {
                        editor.playback.play_from(editor.current_time_secs);
                    } else {
                        editor.playback.pause_at(editor.current_time_secs);
                        outcome.preview_request = Some(editor.current_time_secs);
                    }
                } else if response.dragged() {
                    if is_fast_scrub {
                        outcome.fast_preview_request = Some(editor.current_time_secs);
                    } else {
                        outcome.preview_request = Some(editor.current_time_secs);
                    }
                }
            }
        }
    }

    if response.drag_stopped() {
        if editor.was_playing_before_scrub {
            editor.is_playing = true;
            editor.was_playing_before_scrub = false;
            editor.playback.play_from(editor.current_time_secs);
        } else {
            outcome.preview_request = Some(editor.current_time_secs);
        }
        editor.last_scrub_time = None;
        editor.last_scrub_position = None;
    }
}

pub(super) fn render_editor_workspace_impl(
    ui: &mut egui::Ui,
    editor: &mut EditorState,
    outcome: &mut EditorUiOutcome,
) {
    let available_size = ui.available_size();
    let stacked_layout = available_size.x < EDITOR_STACK_BREAKPOINT;

    if stacked_layout {
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                render_editor_main_panel(ui, editor, outcome);
                ui.add_space(12.0);
                render_editor_sidebar(ui, editor, outcome, false);
            });
        return;
    }

    ui.horizontal_top(|ui| {
        let content_width = available_size.x.min(1820.0);
        let horizontal_gutter = ((available_size.x - content_width) * 0.5).max(0.0);
        if horizontal_gutter > 0.0 {
            ui.add_space(horizontal_gutter);
        }

        let max_sidebar_width = (content_width * 0.34).clamp(EDITOR_SIDEBAR_WIDTH, 460.0);
        let sidebar_width =
            (content_width * 0.28).clamp(EDITOR_SIDEBAR_MIN_WIDTH, max_sidebar_width);
        let main_width = (content_width - sidebar_width - 16.0).max(360.0);

        ui.allocate_ui_with_layout(
            egui::vec2(main_width, available_size.y.max(320.0)),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                render_editor_main_panel(ui, editor, outcome);
            },
        );

        ui.add_space(12.0);

        ui.allocate_ui_with_layout(
            egui::vec2(sidebar_width, available_size.y.max(320.0)),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                render_editor_sidebar(ui, editor, outcome, true);
            },
        );
    });
}

fn render_editor_main_panel(
    ui: &mut egui::Ui,
    editor: &mut EditorState,
    outcome: &mut EditorUiOutcome,
) {
    render_preview_panel_impl(ui, editor, outcome);
    ui.add_space(10.0);
    egui::Frame::group(ui.style()).show(ui, |ui| {
        render_editor_stats(ui, editor);
    });
    ui.add_space(10.0);
    render_timeline_panel(ui, editor, outcome);
}

fn render_editor_sidebar(
    ui: &mut egui::Ui,
    editor: &mut EditorState,
    outcome: &mut EditorUiOutcome,
    fill_height: bool,
) {
    let render_contents =
        |ui: &mut egui::Ui, editor: &mut EditorState, outcome: &mut EditorUiOutcome| {
            render_snippet_list(ui, editor, outcome);
            ui.add_space(10.0);
            render_size_section(ui, editor);
            ui.add_space(10.0);
            render_action_section(ui, editor, outcome);
        };

    if fill_height {
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                render_contents(ui, editor, outcome);
            });
    } else {
        render_contents(ui, editor, outcome);
    }
}

fn render_action_section(
    ui: &mut egui::Ui,
    editor: &mut EditorState,
    outcome: &mut EditorUiOutcome,
) {
    let can_export = !editor.kept_ranges().is_empty() && editor.target_size_mb > 0;

    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.label(egui::RichText::new("Actions").strong());
        ui.add_space(6.0);

        let fallback_output_path = super::build_clipped_output_path(&editor.video);
        let output_path = editor
            .custom_output_path
            .clone()
            .unwrap_or(fallback_output_path);
        ui.label(
            egui::RichText::new(format!("Save To: {}", output_path.display()))
                .small()
                .weak(),
        );
        ui.horizontal_wrapped(|ui| {
            if ui.button("Choose Save Location...").clicked() {
                outcome.request_save_output_dialog = Some(output_path.clone());
            }
            if editor.custom_output_path.is_some() && ui.button("Use Default Path").clicked() {
                editor.custom_output_path = None;
            }
        });
        ui.add_space(6.0);

        if ui
            .add_enabled(
                can_export,
                egui::Button::new("Export Clip (Ctrl+E / Ctrl+S)")
                    .min_size(egui::vec2(ui.available_width(), 32.0)),
            )
            .clicked()
        {
            start_export(editor);
        }
    });
}

fn hit_test_cut_point(editor: &EditorState, rect: egui::Rect, pointer_x: f32) -> Option<usize> {
    let mut best_match = None;
    let mut best_distance = f32::MAX;

    for (index, cut_point) in editor.cut_points.iter().enumerate() {
        let x = super::time_to_x(rect, *cut_point, editor.duration_secs());
        let distance = (pointer_x - x).abs();
        if distance < 8.0 && distance < best_distance {
            best_distance = distance;
            best_match = Some(index);
        }
    }

    best_match
}

fn render_snippet_list(ui: &mut egui::Ui, editor: &mut EditorState, outcome: &mut EditorUiOutcome) {
    clamp_selected_snippet_index_impl(editor);

    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.label(egui::RichText::new("Snippets").strong());
        ui.label(egui::RichText::new("Use the timeline and add cuts at the playhead to split the clip into snippets. Disabled snippets are skipped in preview/export.").weak());

        let snippets = editor.snippets();
        let snippet_max_height = (ui.available_height() * 0.5).clamp(180.0, 420.0);
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .max_height(snippet_max_height)
            .show(ui, |ui| {
                for (index, snippet) in snippets.iter().copied().enumerate() {
                    let snippet_frame = egui::Frame::group(ui.style()).inner_margin(egui::Margin::same(8));

                    snippet_frame
                        .inner_margin(egui::Margin::same(8))
                        .show(ui, |ui| {
                            ui.horizontal_wrapped(|ui| {
                                let mut enabled = editor.snippet_enabled.get(index).copied().unwrap_or(true);
                                if ui.checkbox(&mut enabled, format!("Snippet {}", index + 1)).changed() {
                                    editor.selected_snippet_index = Some(index);
                                    if let Some(flag) = editor.snippet_enabled.get_mut(index) {
                                        *flag = enabled;
                                    }
                                    outcome.preview_request = Some(editor.current_time_secs);
                                }
                                ui.label(format!(
                                    "{} to {} ({})",
                                    format_timestamp_precise(snippet.start_secs),
                                    format_timestamp_precise(snippet.end_secs),
                                    format_compact_duration(snippet.duration_secs()),
                                ));
                                if index < editor.cut_points.len() && ui.button("Remove following cut").clicked() {
                                    editor.selected_snippet_index = Some(index);
                                    remove_cut_point(editor, index);
                                    outcome.preview_request = Some(editor.current_time_secs);
                                }
                            });
                        });
                    ui.add_space(6.0);
                }
            });
    });
}

fn render_size_section(ui: &mut egui::Ui, editor: &mut EditorState) {
    let kept_duration = editor.kept_duration_secs();
    let kept_ranges = editor.kept_ranges();
    let max_output_size_mb = editor.max_output_size_mb();
    let total_duration = editor.duration_secs();

    // Compute the effective auto target size based on kept proportion
    let kept_proportion = if total_duration > 0.0 {
        (kept_duration / total_duration).clamp(0.0, 1.0)
    } else {
        1.0
    };
    let auto_target_size_mb = (editor.video.size_mb * kept_proportion).ceil().max(1.0) as u32;

    // Clamp target size to max allowed based on enabled segments
    if editor.target_size_mb > max_output_size_mb {
        editor.target_size_mb = max_output_size_mb;
    }

    let (video_kbps, total_kbps) = estimate_export_bitrates_from_editor(
        editor.target_size_mb,
        kept_duration,
        editor.video.metadata.has_audio,
        editor.audio_bitrate_kbps,
        kept_ranges.len(),
        editor.use_hardware_acceleration,
    );
    let (quality_label, bars) = super::quality_estimate(&editor.video.metadata, video_kbps);

    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.label(egui::RichText::new("Export Settings").strong());
        ui.add_space(6.0);

        // Show stream copy mode when size hasn't been manually adjusted
        if !editor.target_size_manually_adjusted {
            ui.horizontal_wrapped(|ui| {
                ui.label("Output Size:");
                ui.label(
                    egui::RichText::new(format!("{} MB (auto)", auto_target_size_mb)).strong(),
                );
            });
            ui.label(
                egui::RichText::new(
                    "Stream copy mode: no re-encoding (fastest, preserves quality)",
                )
                .color(egui::Color32::from_rgb(100, 200, 100)),
            );
            if ui.button("Adjust size to enable compression").clicked() {
                editor.target_size_manually_adjusted = true;
            }
        } else {
            ui.horizontal_wrapped(|ui| {
                ui.label("Target Output Size:");
                let prev_size = editor.target_size_mb;
                ui.add(
                    egui::DragValue::new(&mut editor.target_size_mb)
                        .range(1..=max_output_size_mb)
                        .suffix(" MB")
                        .speed(1),
                );
                // Mark as manually adjusted if the value changed
                if editor.target_size_mb != prev_size {
                    editor.target_size_manually_adjusted = true;
                }
            });
            ui.label(
                egui::RichText::new(format!(
                    "Max size for kept segments: {} MB",
                    max_output_size_mb
                ))
                .small()
                .weak(),
            );

            // Button to revert to stream copy mode
            if ui.button("Use stream copy (no compression)").clicked() {
                editor.target_size_manually_adjusted = false;
                // Reset to auto-calculated size
                editor.target_size_mb = DEFAULT_TARGET_SIZE_MB
                    .max(editor.video.size_mb.round() as u32 / 2)
                    .min(editor.video.size_mb.ceil().max(1.0) as u32)
                    .min(max_output_size_mb);
            }

            if editor.video.metadata.has_audio {
                ui.horizontal_wrapped(|ui| {
                    ui.label("Audio Bitrate:");
                    ui.add(
                        egui::DragValue::new(&mut editor.audio_bitrate_kbps)
                            .range(48..=320)
                            .suffix(" kbps")
                            .speed(1),
                    );
                });
            }
            ui.checkbox(
                &mut editor.use_hardware_acceleration,
                "Use hardware acceleration (fallback to software if unavailable)",
            );

            ui.add_space(8.0);
            ui.separator();
            ui.add_space(4.0);

            // Resolution settings
            ui.horizontal(|ui| {
                ui.checkbox(&mut editor.use_auto_resolution, "Auto resolution");
                if editor.use_auto_resolution {
                    let (auto_w, auto_h) = editor.effective_output_resolution();
                    ui.label(egui::RichText::new(format!("-> {}x{}", auto_w, auto_h)).small());
                }
            });

            if !editor.use_auto_resolution {
                ui.horizontal(|ui| {
                    ui.label("Resolution:");
                    let mut w = editor.output_width.unwrap_or(editor.video.metadata.width);
                    let mut h = editor.output_height.unwrap_or(editor.video.metadata.height);
                    ui.add(
                        egui::DragValue::new(&mut w)
                            .range(320..=3840)
                            .suffix("px")
                            .speed(2),
                    );
                    ui.label("×");
                    ui.add(
                        egui::DragValue::new(&mut h)
                            .range(240..=2160)
                            .suffix("px")
                            .speed(2),
                    );
                    editor.output_width = Some(w);
                    editor.output_height = Some(h);
                });
            }

            // FPS settings
            ui.horizontal(|ui| {
                ui.label("Frame Rate:");
                let mut fps = editor.output_fps.unwrap_or(editor.video.metadata.fps);
                ui.add(
                    egui::DragValue::new(&mut fps)
                        .range(15.0..=120.0)
                        .suffix(" fps")
                        .speed(1),
                );
                if ui.button("Original").clicked() {
                    fps = editor.video.metadata.fps;
                }
                editor.output_fps = Some(fps);
            });

            ui.label(format!(
                "Estimated Quality: [{}{}] {} (video ~{:.2} Mbps, total ~{:.2} Mbps)",
                "#".repeat(bars),
                "-".repeat(5 - bars),
                quality_label,
                video_kbps as f64 / 1000.0,
                total_kbps as f64 / 1000.0,
            ));
        }

        ui.label(format!(
            "Kept duration after cuts: {}",
            format_compact_duration(kept_duration)
        ));
    });
}
