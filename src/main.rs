mod gui;
mod recorder;
mod settings;

use eframe::egui;
use global_hotkey::{
    hotkey::{Code, HotKey, Modifiers},
    GlobalHotKeyEvent, GlobalHotKeyManager,
};
use std::sync::{Arc, Mutex};

use gui::LiteClipApp;
use recorder::Recorder;
use settings::HotkeyPreset;

fn main() -> eframe::Result<()> {
    // --- Shared recorder state ---
    let recorder = Arc::new(Mutex::new(Recorder::new()));

    // --- Register initial global hotkey ---
    let hotkey_manager = GlobalHotKeyManager::new().expect("Failed to init hotkey manager");
    let initial_hotkey = {
        let rec = recorder.lock().unwrap();
        rec.settings.hotkey
    };
    let current_hotkey = create_hotkey(initial_hotkey);
    hotkey_manager
        .register(current_hotkey)
        .expect("Failed to register hotkey");
    let current_hotkey_id = current_hotkey.id();

    let recorder_for_app = recorder.clone();

    // --- Window options ---
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("LiteClip")
            .with_inner_size([340.0, 320.0])
            .with_min_inner_size([300.0, 260.0])
            .with_max_inner_size([500.0, 500.0])
            .with_always_on_top(),
        ..Default::default()
    };

    eframe::run_native(
        "LiteClip",
        options,
        Box::new(move |_cc| {
            Ok(Box::new(HotkeyWrapper {
                app: LiteClipApp::new(recorder_for_app),
                hotkey_manager,
                current_hotkey,
                current_hotkey_id,
            }))
        }),
    )
}

/// Wraps the LiteClipApp to also poll for global hotkey events
/// and handle dynamic hotkey re-registration.
struct HotkeyWrapper {
    app: LiteClipApp,
    hotkey_manager: GlobalHotKeyManager,
    current_hotkey: HotKey,
    current_hotkey_id: u32,
}

impl eframe::App for HotkeyWrapper {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        // Check for hotkey events
        while let Ok(event) = GlobalHotKeyEvent::receiver().try_recv() {
            if event.id == self.current_hotkey_id {
                self.app.trigger_save();
            }
        }

        // Check if the GUI requested a hotkey change
        if let Some(new_preset) = self.app.pending_hotkey.take() {
            // Unregister old hotkey
            let _ = self.hotkey_manager.unregister(self.current_hotkey);

            // Register new hotkey
            let new_hotkey = create_hotkey(new_preset);
            match self.hotkey_manager.register(new_hotkey) {
                Ok(()) => {
                    self.current_hotkey = new_hotkey;
                    self.current_hotkey_id = new_hotkey.id();
                    self.app.status_message = format!("Hotkey changed to {}", new_preset.label());
                    self.app.status_timer = 3.0;
                }
                Err(e) => {
                    // Re-register old hotkey on failure
                    let _ = self.hotkey_manager.register(self.current_hotkey);
                    self.app.status_message = format!("Hotkey error: {}", e);
                    self.app.status_timer = 5.0;
                }
            }
        }

        // Delegate to the actual app
        self.app.update(ctx, frame);
    }
}

/// Create a HotKey from a HotkeyPreset.
fn create_hotkey(preset: HotkeyPreset) -> HotKey {
    match preset {
        HotkeyPreset::F8 => HotKey::new(None, Code::F8),
        HotkeyPreset::F9 => HotKey::new(None, Code::F9),
        HotkeyPreset::F10 => HotKey::new(None, Code::F10),
        HotkeyPreset::CtrlShiftS => {
            HotKey::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::KeyS)
        }
        HotkeyPreset::AltF9 => HotKey::new(Some(Modifiers::ALT), Code::F9),
    }
}
