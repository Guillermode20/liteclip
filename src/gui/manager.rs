use crate::capture::audio::AudioLevelMonitor;
use crate::platform::AppEvent;
use eframe::egui;
use egui_notify::{Anchor, Toasts};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::LazyLock;
use std::sync::Once;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc::Sender as TokioSender;
#[cfg(target_os = "windows")]
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, GetWindowLongPtrW, SetWindowLongPtrW, GWL_EXSTYLE, SM_CXSCREEN, SM_CYSCREEN,
    WS_EX_NOACTIVATE, WS_EX_TRANSPARENT,
};

pub enum GuiMessage {
    ShowSettings(TokioSender<AppEvent>, Option<AudioLevelMonitor>),
    ShowGallery(TokioSender<AppEvent>),
    Toast(ToastKind, String),
}

pub enum ToastKind {
    Success,
    Error,
    Info,
    Warning,
}

static GUI_TX: LazyLock<Mutex<Option<Sender<GuiMessage>>>> = LazyLock::new(|| Mutex::new(None));
static GUI_INIT: Once = Once::new();

const TOAST_WINDOW_SIZE: [f32; 2] = [350.0, 300.0];
const TOAST_WINDOW_MARGIN: [f32; 2] = [20.0, 20.0];
const IDLE_REPAINT_MS: u64 = 100;
/// GUI Manager for the application.
///
/// Handles the centralized display of toasts and oversees the creation
/// of main GUI windows (Settings and Gallery).
///
/// # Threading
///
/// The GUI manager initializes a dedicated background thread for the `egui`
/// overlay which manages notifications (toasts). Other windows like Settings
/// and Gallery are spawned as needed from their respective modules, each
/// running in its own native thread to avoid stalling the recording pipeline.
pub fn init_gui_manager() {
    GUI_INIT.call_once(|| {
        let (tx, rx) = channel();
        *GUI_TX.lock().unwrap_or_else(|e| e.into_inner()) = Some(tx);

        std::thread::spawn(move || {
            let pos = get_toast_window_pos();

            let options = eframe::NativeOptions {
                renderer: eframe::Renderer::Glow,
                viewport: egui::ViewportBuilder::default()
                    .with_transparent(true)
                    .with_always_on_top()
                    .with_decorations(false)
                    .with_taskbar(false)
                    .with_active(false)
                    .with_inner_size(TOAST_WINDOW_SIZE)
                    .with_position(pos),
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
                "liteclip_overlay",
                options,
                Box::new(|cc| Ok(Box::new(GuiManagerApp::new(cc, rx)))),
            );
        });
    });
}

fn get_toast_window_pos() -> [f32; 2] {
    #[cfg(target_os = "windows")]
    {
        let screen_width = unsafe { GetSystemMetrics(SM_CXSCREEN) }.max(0) as f32;
        let screen_height = unsafe { GetSystemMetrics(SM_CYSCREEN) }.max(0) as f32;

        [
            (screen_width - TOAST_WINDOW_SIZE[0] - TOAST_WINDOW_MARGIN[0]).max(0.0),
            TOAST_WINDOW_MARGIN[1].min((screen_height - TOAST_WINDOW_MARGIN[1]).max(0.0)),
        ]
    }

    #[cfg(not(target_os = "windows"))]
    {
        [TOAST_WINDOW_MARGIN[0], TOAST_WINDOW_MARGIN[1]]
    }
}

pub fn send_gui_message(msg: GuiMessage) {
    init_gui_manager();
    if let Some(tx) = GUI_TX.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        let _ = tx.send(msg);
    }
}

pub fn show_toast(kind: ToastKind, message: impl Into<String>) {
    send_gui_message(GuiMessage::Toast(kind, message.into()));
}

struct GuiManagerApp {
    rx: Receiver<GuiMessage>,
    settings: Arc<Mutex<Option<crate::gui::settings::SettingsApp>>>,
    gallery: Arc<Mutex<Option<crate::gui::gallery::GalleryApp>>>,
    toasts: Toasts,
    #[cfg(target_os = "windows")]
    hwnd: Option<windows::Win32::Foundation::HWND>,
}

impl GuiManagerApp {
    #[cfg(target_os = "windows")]
    fn new(cc: &eframe::CreationContext<'_>, rx: Receiver<GuiMessage>) -> Self {
        use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let hwnd = if let Ok(handle) = cc.window_handle() {
            if let RawWindowHandle::Win32(win32) = handle.as_raw() {
                Some(windows::Win32::Foundation::HWND(win32.hwnd.get() as _))
            } else {
                None
            }
        } else {
            None
        };

        Self {
            rx,
            settings: Arc::new(Mutex::new(None)),
            gallery: Arc::new(Mutex::new(None)),
            toasts: Toasts::default().with_anchor(Anchor::TopRight),
            hwnd,
        }
    }

    #[cfg(not(target_os = "windows"))]
    fn new(_cc: &eframe::CreationContext<'_>, rx: Receiver<GuiMessage>) -> Self {
        let mut visuals = cc.egui_ctx.style().visuals.clone();
        visuals.selection.bg_fill = egui::Color32::TRANSPARENT;
        visuals.selection.stroke = egui::Stroke::new(0.0, egui::Color32::TRANSPARENT);
        cc.egui_ctx.set_visuals(visuals);

        Self {
            rx,
            settings: Arc::new(Mutex::new(None)),
            gallery: Arc::new(Mutex::new(None)),
            toasts: Toasts::default().with_anchor(Anchor::TopRight),
        }
    }

    #[cfg(target_os = "windows")]
    fn set_click_through(&self, click_through: bool) {
        if let Some(hwnd) = self.hwnd {
            unsafe {
                let ex_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
                if click_through {
                    SetWindowLongPtrW(
                        hwnd,
                        GWL_EXSTYLE,
                        ex_style | WS_EX_TRANSPARENT.0 as isize | WS_EX_NOACTIVATE.0 as isize,
                    );
                } else {
                    SetWindowLongPtrW(
                        hwnd,
                        GWL_EXSTYLE,
                        (ex_style & !(WS_EX_TRANSPARENT.0 as isize)) | WS_EX_NOACTIVATE.0 as isize,
                    );
                }
            }
        }
    }

    #[cfg(not(target_os = "windows"))]
    fn set_click_through(&self, _click_through: bool) {}
}

impl eframe::App for GuiManagerApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        egui::Rgba::TRANSPARENT.to_array()
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                GuiMessage::ShowSettings(tx, level_monitor) => {
                    let config = crate::config::Config::load_sync().unwrap_or_default();
                    *self.settings.lock().unwrap_or_else(|e| e.into_inner()) = Some(crate::gui::settings::SettingsApp::new(
                        config,
                        tx,
                        level_monitor,
                    ));
                }
                GuiMessage::ShowGallery(tx) => {
                    let config = crate::config::Config::load_sync().unwrap_or_default();
                    *self.gallery.lock().unwrap_or_else(|e| e.into_inner()) =
                        Some(crate::gui::gallery::GalleryApp::new(&config, tx));
                }
                GuiMessage::Toast(kind, message) => {
                    match kind {
                        ToastKind::Success => {
                            self.toasts
                                .success(message)
                                .duration(Duration::from_secs(3));
                        }
                        ToastKind::Error => {
                            self.toasts.error(message).duration(Duration::from_secs(5));
                        }
                        ToastKind::Info => {
                            self.toasts.info(message).duration(Duration::from_secs(3));
                        }
                        ToastKind::Warning => {
                            self.toasts
                                .warning(message)
                                .duration(Duration::from_secs(4));
                        }
                    }
                    ctx.request_repaint();
                }
            }
        }

        let frame = egui::Frame::NONE.fill(egui::Color32::TRANSPARENT);
        egui::CentralPanel::default()
            .frame(frame)
            .show(ctx, |_ui| {});

        let settings_clone = self.settings.clone();
        let show_settings = settings_clone.lock().unwrap_or_else(|e| e.into_inner()).is_some();
        if show_settings {
            ctx.show_viewport_deferred(
                egui::ViewportId::from_hash_of("settings"),
                egui::ViewportBuilder::default()
                    .with_title("LiteClip Replay Settings")
                    .with_inner_size([600.0, 700.0])
                    .with_resizable(true)
                    .with_min_inner_size([600.0, 500.0]),
                move |ctx, class| {
                    if class == egui::ViewportClass::Embedded {
                        return;
                    }

                    let mut lock = settings_clone.lock().unwrap_or_else(|e| e.into_inner());
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
        let show_gallery = gallery_clone.lock().unwrap_or_else(|e| e.into_inner()).is_some();
        if show_gallery {
            ctx.show_viewport_deferred(
                egui::ViewportId::from_hash_of("gallery"),
                egui::ViewportBuilder::default()
                    .with_title("LiteClip Clip & Compress")
                    .with_inner_size([1280.0, 820.0])
                    .with_resizable(true)
                    .with_min_inner_size([720.0, 520.0]),
                move |ctx, class| {
                    if class == egui::ViewportClass::Embedded {
                        return;
                    }

                    let mut lock = gallery_clone.lock().unwrap_or_else(|e| e.into_inner());
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

        if show_settings || show_gallery {
            ctx.request_repaint();
        } else {
            ctx.request_repaint_after(Duration::from_millis(IDLE_REPAINT_MS));
        }

        self.set_click_through(true);

        self.toasts.show(ctx);
    }
}
