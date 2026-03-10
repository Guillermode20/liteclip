use crate::platform::AppEvent;
use eframe::egui;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::LazyLock;
use std::sync::Once;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::Sender as TokioSender;

use crate::config::OverlayPosition;

pub enum GuiMessage {
    ShowSettings(TokioSender<AppEvent>),
    ShowGallery(TokioSender<AppEvent>),
    ShowOverlay(Option<String>),
}

static GUI_TX: LazyLock<Mutex<Option<Sender<GuiMessage>>>> = LazyLock::new(|| Mutex::new(None));
static GUI_INIT: Once = Once::new();

pub fn init_gui_manager() {
    GUI_INIT.call_once(|| {
        let (tx, rx) = channel();
        *GUI_TX.lock().unwrap() = Some(tx);

        std::thread::spawn(move || {
            let options = eframe::NativeOptions {
                // The GUI manager mostly hosts small settings/gallery windows and a tiny
                // clip-saved overlay. Auto-selecting wgpu here spins up a full graphics
                // backend on first successful save, which keeps a large amount of graphics
                // memory resident for the rest of the session. Prefer Glow to keep this
                // helper thread lightweight.
                renderer: eframe::Renderer::Glow,
                viewport: egui::ViewportBuilder::default()
                    .with_visible(false)
                    .with_active(false)
                    .with_position([-10000.0, -10000.0])
                    .with_taskbar(false)
                    .with_decorations(false)
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
    });
}

pub fn send_gui_message(msg: GuiMessage) {
    init_gui_manager();
    if let Some(tx) = GUI_TX.lock().unwrap().as_ref() {
        let _ = tx.send(msg);
    }
}

struct OverlayState {
    filename: Option<String>,
    shown_at: Instant,
    position: OverlayPosition,
}

struct GuiManagerApp {
    rx: Receiver<GuiMessage>,
    settings: Arc<Mutex<Option<crate::gui::settings::SettingsApp>>>,
    gallery: Arc<Mutex<Option<crate::gui::gallery::GalleryApp>>>,
    overlay: Arc<Mutex<Option<OverlayState>>>,
    overlay_position: OverlayPosition,
    transparent_overlay_supported: bool,
}

impl GuiManagerApp {
    fn new(_cc: &eframe::CreationContext<'_>, rx: Receiver<GuiMessage>) -> Self {
        let overlay_position = crate::config::Config::load_sync()
            .map(|c| c.advanced.overlay_position)
            .unwrap_or(OverlayPosition::TopLeft);

        Self {
            rx,
            settings: Arc::new(Mutex::new(None)),
            gallery: Arc::new(Mutex::new(None)),
            overlay: Arc::new(Mutex::new(None)),
            overlay_position,
            transparent_overlay_supported: false,
        }
    }
}

impl eframe::App for GuiManagerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                GuiMessage::ShowSettings(tx) => {
                    let config = crate::config::Config::load_sync().unwrap_or_default();
                    *self.settings.lock().unwrap() =
                        Some(crate::gui::settings::SettingsApp::new(config, tx));
                }
                GuiMessage::ShowGallery(tx) => {
                    let config = crate::config::Config::load_sync().unwrap_or_default();
                    *self.gallery.lock().unwrap() =
                        Some(crate::gui::gallery::GalleryApp::new(&config, tx));
                }
                GuiMessage::ShowOverlay(filename) => {
                    *self.overlay.lock().unwrap() = Some(OverlayState {
                        filename,
                        shown_at: Instant::now(),
                        position: self.overlay_position,
                    });
                }
            }
        }

        let settings_clone = self.settings.clone();
        let show_settings = settings_clone.lock().unwrap().is_some();
        if show_settings {
            ctx.show_viewport_deferred(
                egui::ViewportId::from_hash_of("settings"),
                egui::ViewportBuilder::default()
                    .with_title("LiteClip Replay Settings")
                    .with_inner_size([600.0, 700.0])
                    .with_resizable(true),
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

        let gallery_clone = self.gallery.clone();
        let show_gallery = gallery_clone.lock().unwrap().is_some();
        if show_gallery {
            ctx.show_viewport_deferred(
                egui::ViewportId::from_hash_of("gallery"),
                egui::ViewportBuilder::default()
                    .with_title("LiteClip Gallery")
                    .with_inner_size([900.0, 600.0])
                    .with_resizable(true)
                    .with_min_inner_size([400.0, 300.0]),
                move |ctx, class| {
                    if class == egui::ViewportClass::Embedded {
                        return;
                    }

                    let mut lock = gallery_clone.lock().unwrap();
                    if let Some(gallery) = lock.as_mut() {
                        let mut is_open = true;
                        gallery.update(ctx, &mut is_open);
                        if ctx.input(|i| i.viewport().close_requested()) {
                            *lock = None;
                        }
                    }
                },
            );
        }

        let overlay_clone = self.overlay.clone();
        let overlay_active = overlay_clone.lock().unwrap().is_some();
        let overlay_position = overlay_clone.lock().unwrap().as_ref().map(|o| o.position);
        let transparent_overlay_supported = self.transparent_overlay_supported;
        if overlay_active {
            ctx.show_viewport_deferred(
                egui::ViewportId::from_hash_of("overlay"),
                egui::ViewportBuilder::default()
                    .with_decorations(false)
                    .with_always_on_top()
                    .with_taskbar(false)
                    .with_transparent(transparent_overlay_supported)
                    .with_inner_size([200.0, 64.0])
                    .with_resizable(false)
                    .with_position(get_overlay_position(overlay_position)),
                move |ctx, class| {
                    if class == egui::ViewportClass::Embedded {
                        return;
                    }

                    let mut lock = overlay_clone.lock().unwrap();
                    if let Some(state) = lock.as_ref() {
                        let elapsed = state.shown_at.elapsed().as_secs_f32();
                        let display_duration = 2.5;

                        if elapsed >= display_duration {
                            *lock = None;
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            return;
                        }

                        crate::gui::clip_saved_overlay::render_overlay_direct(
                            ctx,
                            &state.filename,
                            state.shown_at,
                            display_duration,
                            transparent_overlay_supported,
                        );
                    }
                },
            );
        }

        if show_settings || show_gallery || overlay_active {
            ctx.request_repaint();
        } else {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
    }
}

fn get_overlay_position(position: Option<OverlayPosition>) -> egui::Pos2 {
    use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};

    let (screen_w, screen_h) = unsafe {
        (
            GetSystemMetrics(SM_CXSCREEN) as f32,
            GetSystemMetrics(SM_CYSCREEN) as f32,
        )
    };

    let overlay_w = 200.0;
    let overlay_h = 60.0;
    let margin = 20.0;

    match position.unwrap_or(OverlayPosition::TopLeft) {
        OverlayPosition::TopLeft => egui::Pos2::new(margin, margin),
        OverlayPosition::TopRight => egui::Pos2::new(screen_w - overlay_w - margin, margin),
        OverlayPosition::BottomLeft => egui::Pos2::new(margin, screen_h - overlay_h - margin),
        OverlayPosition::BottomRight => {
            egui::Pos2::new(screen_w - overlay_w - margin, screen_h - overlay_h - margin)
        }
    }
}
