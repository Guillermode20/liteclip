use eframe::egui;

use super::{
    add_cut_point, clamp_selected_snippet_index, poll_editor_export_updates, remove_cut_point,
    render_completion_screen, render_editor_workspace, seek_editor, start_export,
    toggle_editor_playback, update_playback_clock, ClipCompressApp, EditorUiOutcome,
    EDITOR_LARGE_SEEK_SECS, EDITOR_SMALL_SEEK_SECS,
};

pub(super) fn render_editor_ui(app: &mut ClipCompressApp, ui: &mut egui::Ui) -> EditorUiOutcome {
    let mut outcome = EditorUiOutcome::default();
    let Some(editor) = app.editor.as_mut() else {
        return outcome;
    };

    let ctx = ui.ctx().clone();
    let received_update = poll_editor_export_updates(editor, &mut outcome, &ctx);

    let export_active = editor.has_active_export();

    // Render export progress modal overlay on top of everything when active
    if export_active {
        outcome = render_export_modal(ui, editor, outcome, &ctx);
    }

    if editor.export_output.is_some() {
        return render_completion_screen(ui, editor);
    }

    update_playback_clock(editor, &mut outcome);

    handle_editor_shortcuts(&ctx, editor, &mut outcome, export_active);

    ui.horizontal(|ui| {
        if ui
            .add_enabled(!export_active, egui::Button::new("< Back to Videos (Esc)"))
            .clicked()
        {
            outcome.back_to_browser = true;
        }
        ui.heading(format!("Editing: {}", editor.video.filename));
        ui.separator();
        ui.label(egui::RichText::new(
            "Hotkeys: Space=Play/Pause · ←/→=Seek · Home/End=Jump · A=Add cut · Del=Remove cut · Esc=Back · Ctrl+E/S=Export · Ctrl+Z=Undo",
        )
        .weak()
        .small());
    });
    ui.separator();

    ui.add_enabled_ui(!export_active, |ui| {
        render_editor_workspace(ui, editor, &mut outcome);
    });

    if let Some(status) = &editor.status_message {
        ui.add_space(6.0);
        ui.colored_label(egui::Color32::LIGHT_GREEN, status);
    }
    if let Some(error) = &editor.error_message {
        ui.add_space(6.0);
        ui.colored_label(egui::Color32::LIGHT_RED, error);
    }

    // Request immediate repaint if we received an update or export is active
    if received_update || export_active {
        ctx.request_repaint();
    }

    outcome
}

fn render_export_modal(
    ui: &mut egui::Ui,
    editor: &mut super::EditorState,
    outcome: EditorUiOutcome,
    _ctx: &egui::Context,
) -> EditorUiOutcome {
    let Some(export) = editor.export_state.as_mut() else {
        return outcome;
    };

    let ctx = ui.ctx().clone();

    // Create a centered modal window using egui's built-in anchoring
    egui::Window::new("export_modal")
        .title_bar(false)
        .resizable(false)
        .collapsible(false)
        .movable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .frame(egui::Frame::window(ui.style()).corner_radius(8.0))
        .show(ui.ctx(), |ui| {
            ui.set_min_width(360.0);
            ui.vertical_centered(|ui| {
                ui.add_space(8.0);
                ui.heading("Exporting Clip");
                ui.add_space(12.0);

                // Progress bar with animated styling
                let progress = export.progress.clamp(0.0, 1.0);
                let progress_bar = egui::ProgressBar::new(progress)
                    .show_percentage()
                    .desired_width(320.0);
                ui.add(progress_bar);

                ui.add_space(6.0);

                let text_color = ui.visuals().widgets.inactive.text_color();
                ui.label(
                    egui::RichText::new(&export.message)
                        .size(14.0)
                        .color(text_color),
                );

                ui.add_space(12.0);

                // Cancel button centered
                if ui
                    .button(egui::RichText::new("Cancel Export").size(14.0))
                    .clicked()
                {
                    export
                        .cancel_flag
                        .store(true, std::sync::atomic::Ordering::SeqCst);
                    export.message = "Cancelling export...".to_string();
                }

                ui.add_space(4.0);
            });
        });

    // Request smooth animation for progress updates - every 16ms (~60fps)
    ctx.request_repaint_after(std::time::Duration::from_millis(16));

    outcome
}

fn handle_editor_shortcuts(
    ctx: &egui::Context,
    editor: &mut super::EditorState,
    outcome: &mut EditorUiOutcome,
    export_active: bool,
) {
    if ctx.wants_keyboard_input() {
        return;
    }

    if export_active {
        return;
    }

    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        outcome.back_to_browser = true;
    }

    // Export can be triggered from either Ctrl+E or Ctrl+S (Cmd on macOS)
    if ctx.input(|i| {
        i.modifiers.command && (i.key_pressed(egui::Key::E) || i.key_pressed(egui::Key::S))
    }) && !editor.kept_ranges().is_empty()
        && editor.target_size_mb > 0
    {
        start_export(editor);
    }

    // Undo last cut point (basic undo)
    if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Z)) {
        if !editor.cut_points.is_empty() {
            editor.cut_points.pop();
            if let Some(selected) = editor.selected_cut_point {
                if selected >= editor.cut_points.len() {
                    editor.selected_cut_point = editor.cut_points.len().checked_sub(1);
                }
            }
            outcome.preview_request = Some(editor.current_time_secs);
        }
    }

    if ctx.input(|i| i.key_pressed(egui::Key::Space)) {
        toggle_editor_playback(editor);
    }

    if ctx.input(|i| i.key_pressed(egui::Key::A)) && add_cut_point(editor, editor.current_time_secs)
    {
        outcome.preview_request = Some(editor.current_time_secs);
    }

    if ctx.input(|i| i.key_pressed(egui::Key::Delete)) {
        if let Some(index) = editor.selected_cut_point {
            remove_cut_point(editor, index);
            outcome.preview_request = Some(editor.current_time_secs);
        }
    }

    if ctx.input(|i| i.key_pressed(egui::Key::ArrowLeft)) {
        let step = if ctx.input(|i| i.modifiers.shift) {
            -EDITOR_LARGE_SEEK_SECS
        } else {
            -EDITOR_SMALL_SEEK_SECS
        };
        seek_editor(editor, outcome, step);
    }

    if ctx.input(|i| i.key_pressed(egui::Key::ArrowRight)) {
        let step = if ctx.input(|i| i.modifiers.shift) {
            EDITOR_LARGE_SEEK_SECS
        } else {
            EDITOR_SMALL_SEEK_SECS
        };
        seek_editor(editor, outcome, step);
    }

    if ctx.input(|i| i.key_pressed(egui::Key::Home)) {
        editor.playback.pause_at(0.0);
        editor.is_playing = false;
        editor.current_time_secs = 0.0;
        outcome.preview_request = Some(0.0);
    }

    if ctx.input(|i| i.key_pressed(egui::Key::End)) {
        let end_time = editor.duration_secs();
        editor.playback.pause_at(end_time);
        editor.is_playing = false;
        editor.current_time_secs = end_time;
        outcome.preview_request = Some(end_time);
    }

    clamp_selected_snippet_index(editor);
    let snippet_count = editor.snippets().len();
    if snippet_count == 0 {
        return;
    }

    let current = editor
        .selected_snippet_index
        .unwrap_or(0)
        .min(snippet_count - 1);
    editor.selected_snippet_index = Some(current);

    if ctx.input(|i| i.key_pressed(egui::Key::ArrowDown)) {
        editor.selected_snippet_index = Some((current + 1).min(snippet_count - 1));
    }

    if ctx.input(|i| i.key_pressed(egui::Key::ArrowUp)) {
        editor.selected_snippet_index = Some(current.saturating_sub(1));
    }

    let selected = editor
        .selected_snippet_index
        .unwrap_or(0)
        .min(snippet_count - 1);

    if ctx.input(|i| i.key_pressed(egui::Key::Space)) {
        if let Some(flag) = editor.snippet_enabled.get_mut(selected) {
            *flag = !*flag;

            // Ensure playback time stays on an enabled snippet after toggling.
            let clamped_time = crate::gui::gallery::clamp_to_enabled_playback_time(
                editor.current_time_secs,
                editor.duration_secs(),
                &editor.cut_points,
                &editor.snippet_enabled,
            );
            editor.current_time_secs = clamped_time;
            editor.playback.pause_at(clamped_time);

            outcome.preview_request = Some(editor.current_time_secs);
        }
    }

    if ctx.input(|i| i.key_pressed(egui::Key::Delete)) && selected < editor.cut_points.len() {
        remove_cut_point(editor, selected);
        outcome.preview_request = Some(editor.current_time_secs);
    }
}
