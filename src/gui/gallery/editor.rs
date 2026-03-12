use eframe::egui;

use super::{
    poll_editor_export_updates, render_completion_screen, render_editor_workspace,
    update_playback_clock, ClipCompressApp, EditorUiOutcome,
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
    ui.horizontal(|ui| {
        if ui
            .add_enabled(!export_active, egui::Button::new("< Back to Videos"))
            .clicked()
        {
            outcome.back_to_browser = true;
        }
        ui.heading(format!("Editing: {}", editor.video.filename));
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
