use eframe::egui;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::sync::LazyLock;
use crate::platform::AppEvent;
use tokio::sync::mpsc::Sender as TokioSender;

pub enum GuiMessage {
    ShowSettings(TokioSender<AppEvent>),
}

static GUI_TX: LazyLock<Mutex<Option<Sender<GuiMessage>>>> = LazyLock::new(|| Mutex::new(None));

/// Initialize the global GUI manager thread.
/// This runs a single persistent eframe event loop to avoid "RecreationAttempt" errors.
pub fn init_gui_manager() {
    let (tx, rx) = channel();
    *GUI_TX.lock().unwrap() = Some(tx);

    std::thread::spawn(move || {
        let options = eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_visible(true) // Visible but offscreen to keep WGPU rendering loop active
                .with_active(false)
                .with_position([-10000.0, -10000.0])
                .with_taskbar(false)
                .with_decorations(false)
                .with_transparent(true)
                .with_inner_size([1.0, 1.0]),
            event_loop_builder: Some(Box::new(|builder| {
                #[cfg(target_os = "windows")]
                {
                    use winit::platform::windows::EventLoopBuilderExtWindows;
                    builder.with_any_thread(true);
                }
            })),
            ..Default::default()
        };

        let _ = eframe::run_native(
            "liteclip_gui_manager",
            options,
            Box::new(|cc| Ok(Box::new(GuiManagerApp::new(cc, rx)))),
        );
    });
}

pub fn send_gui_message(msg: GuiMessage) {
    if let Some(tx) = GUI_TX.lock().unwrap().as_ref() {
        let _ = tx.send(msg);
    }
}

struct GuiManagerApp {
    rx: Receiver<GuiMessage>,
    settings: Arc<Mutex<Option<crate::gui::settings::SettingsApp>>>,
}

impl GuiManagerApp {
    fn new(_cc: &eframe::CreationContext<'_>, rx: Receiver<GuiMessage>) -> Self {
        Self {
            rx,
            settings: Arc::new(Mutex::new(None)),
        }
    }
}

impl eframe::App for GuiManagerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Poll for new window requests
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                GuiMessage::ShowSettings(tx) => {
                    let config = match crate::config::Config::load_sync() {
                        Ok(c) => c,
                        Err(_) => crate::config::Config::default(),
                    };
                    *self.settings.lock().unwrap() = Some(crate::gui::settings::SettingsApp {
                        config,
                        event_tx: tx,
                        save_status: None,
                    });
                }
            }
        }

        // Render Settings if requested
        let settings_clone = self.settings.clone();
        let show_settings = settings_clone.lock().unwrap().is_some();
        if show_settings {
            ctx.show_viewport_deferred(
                egui::ViewportId::from_hash_of("settings"),
                egui::ViewportBuilder::default()
                    .with_title("LiteClip Replay Settings")
                    .with_inner_size([600.0, 700.0]),
                move |ctx, class| {
                    if class == egui::ViewportClass::Embedded {
                        return;
                    }
                    
                    let mut lock = settings_clone.lock().unwrap();
                    if let Some(settings) = lock.as_mut() {
                        let mut is_open = true;
                        settings.update(ctx, &mut is_open);
                        if !is_open || ctx.input(|i| i.viewport().close_requested()) {
                            *lock = None;
                        }
                    }
                },
            );
        }
        
        ctx.request_repaint();
    }
}
