use eframe::egui;

use super::{
    add_cut_point, clamp_selected_snippet_index, cycle_editor_focus_zone,
    poll_editor_export_updates, remove_cut_point, render_completion_screen,
    render_editor_workspace, seek_editor, start_export, toggle_editor_playback,
    update_playback_clock, ClipCompressApp, EditorFocusZone, EditorUiOutcome,
    EDITOR_LARGE_SEEK_SECS, EDITOR_SMALL_SEEK_SECS,
};

pub(super) fn render_editor_ui(app: &mut ClipCompressApp, ui: &mut egui::Ui) -> EditorUiOutcome {
    let mut outcome = EditorUiOutcome::default();
    let Some(editor) = app.editor.as_mut() else {
        return outcome;
    };

    poll_editor_export_updates(editor, &mut outcome);
    if editor.export_output.is_some() {
        return render_completion_screen(ui, editor);
    }

    update_playback_clock(editor, &mut outcome);

    let export_active = editor.has_active_export();
    handle_editor_shortcuts(ui.ctx(), editor, &mut outcome, export_active);

    ui.horizontal(|ui| {
        if ui
            .add_enabled(!export_active, egui::Button::new("< Back to Videos (Esc)"))
            .clicked()
        {
            outcome.back_to_browser = true;
        }
        ui.heading(format!("Editing: {}", editor.video.filename));
        ui.separator();
        ui.label(match editor.focus_zone {
            EditorFocusZone::MainPanel => "Keyboard Focus: Main Panel (Tab to switch)",
            EditorFocusZone::Sidebar => "Keyboard Focus: Sidebar (Tab to switch)",
        });
        ui.label(egui::RichText::new(
            "Hotkeys: Space=Play/Pause · ←/→=Seek · Home/End=Jump · A=Add cut · Del=Remove cut · Tab=Switch focus · Esc=Back · Ctrl+E/S=Export · Ctrl+Z=Undo",
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

    if let Some(export) = editor.export_state.as_mut() {
        ui.add_space(10.0);
        ui.separator();
        ui.label(egui::RichText::new("Exporting clip").strong());
        ui.add(egui::ProgressBar::new(export.progress).show_percentage());
        ui.label(&export.message);
        if ui.button("Cancel Export").clicked() {
            export
                .cancel_flag
                .store(true, std::sync::atomic::Ordering::SeqCst);
            export.message = "Cancelling export...".to_string();
        }
    }

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

    if ctx.input(|i| i.key_pressed(egui::Key::Tab)) {
        let backwards = ctx.input(|i| i.modifiers.shift);
        cycle_editor_focus_zone(editor, backwards);
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

    if editor.focus_zone == EditorFocusZone::MainPanel {
        if ctx.input(|i| i.key_pressed(egui::Key::Space)) {
            toggle_editor_playback(editor);
        }

        if ctx.input(|i| i.key_pressed(egui::Key::A))
            && add_cut_point(editor, editor.current_time_secs)
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

        return;
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
            outcome.preview_request = Some(editor.current_time_secs);
        }
    }

    if ctx.input(|i| i.key_pressed(egui::Key::Delete)) && selected < editor.cut_points.len() {
        remove_cut_point(editor, selected);
        outcome.preview_request = Some(editor.current_time_secs);
    }
}
