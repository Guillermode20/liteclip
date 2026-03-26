use crate::capture::audio::AudioLevelMonitor;
use crate::platform::AppEvent;
use eframe::egui;
use egui_notify::{Anchor, Toasts};
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::sync::LazyLock;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc::Sender as TokioSender;
use tracing::warn;
#[cfg(target_os = "windows")]
use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};

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

#[derive(Default)]
struct GuiManagerState {
    tx: Option<Sender<GuiMessage>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

static GUI_STATE: LazyLock<Mutex<GuiManagerState>> =
    LazyLock::new(|| Mutex::new(GuiManagerState::default()));

const TOAST_WINDOW_SIZE: [f32; 2] = [350.0, 300.0];
/// When idle, shrink the overlay so it does not block clicks elsewhere (1×1 logical pixel).
const TOAST_WINDOW_IDLE_SIZE: [f32; 2] = [1.0, 1.0];
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
    with_gui_state(|state| state.ensure_running());
}

fn spawn_gui_manager_thread(state: &mut GuiManagerState) {
    let (tx, rx) = channel();
    state.tx = Some(tx);

    state.thread = Some(std::thread::spawn(move || {
        let pos = get_toast_window_pos_for_size(TOAST_WINDOW_IDLE_SIZE);

        let options = eframe::NativeOptions {
            renderer: eframe::Renderer::Glow,
            viewport: egui::ViewportBuilder::default()
                .with_transparent(true)
                .with_always_on_top()
                .with_decorations(false)
                .with_taskbar(false)
                .with_active(false)
                .with_inner_size(TOAST_WINDOW_IDLE_SIZE)
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

        if let Err(e) = eframe::run_native(
            "liteclip_overlay",
            options,
            Box::new(|cc| Ok(Box::new(GuiManagerApp::new(cc, rx)))),
        ) {
            warn!("eframe::run_native failed: {:?}", e);
        }
    }));
}

fn with_gui_state<T>(f: impl FnOnce(&mut GuiManagerState) -> T) -> T {
    let mut state = GUI_STATE.lock().unwrap_or_else(|e| e.into_inner());
    f(&mut state)
}

impl GuiManagerState {
    fn cleanup_finished_thread(&mut self) {
        let finished = self
            .thread
            .as_ref()
            .is_some_and(std::thread::JoinHandle::is_finished);

        if !finished {
            return;
        }

        if let Some(thread) = self.thread.take() {
            if let Err(err) = thread.join() {
                warn!("GUI manager thread panicked: {:?}", err);
            }
        }

        self.tx = None;
    }

    fn ensure_running(&mut self) {
        self.cleanup_finished_thread();

        if self.thread.is_none() {
            spawn_gui_manager_thread(self);
        }
    }

    fn request_shutdown(&mut self) {
        self.tx = None;
    }
}

fn get_toast_window_pos_for_size(size: [f32; 2]) -> [f32; 2] {
    #[cfg(target_os = "windows")]
    {
        let screen_width = unsafe { GetSystemMetrics(SM_CXSCREEN) }.max(0) as f32;
        let screen_height = unsafe { GetSystemMetrics(SM_CYSCREEN) }.max(0) as f32;

        [
            (screen_width - size[0] - TOAST_WINDOW_MARGIN[0]).max(0.0),
            TOAST_WINDOW_MARGIN[1].min((screen_height - TOAST_WINDOW_MARGIN[1]).max(0.0)),
        ]
    }

    #[cfg(not(target_os = "windows"))]
    {
        [TOAST_WINDOW_MARGIN[0], TOAST_WINDOW_MARGIN[1]]
    }
}

pub fn send_gui_message(msg: GuiMessage) {
    let mut msg = Some(msg);

    for _ in 0..50 {
        init_gui_manager();

        let tx = with_gui_state(|state| state.tx.as_ref().cloned());
        if let Some(tx) = tx {
            match tx.send(msg.take().expect("GUI message missing during retry")) {
                Ok(()) => return,
                Err(err) => {
                    msg = Some(err.0);
                    with_gui_state(|state| state.request_shutdown());
                }
            }
        } else {
            let thread_still_running = with_gui_state(|state| state.thread.is_some());
            if thread_still_running {
                std::thread::sleep(std::time::Duration::from_millis(10));
                continue;
            }
        }
    }

    if let Some(message) = msg {
        warn!("GUI manager unavailable after waiting for restart; dropping message");
        let _ = message;
    }
}

pub fn show_toast(kind: ToastKind, message: impl Into<String>) {
    send_gui_message(GuiMessage::Toast(kind, message.into()));
}

pub fn shutdown_gui() {
    with_gui_state(|state| state.request_shutdown());
}

struct GuiManagerApp {
    rx: Receiver<GuiMessage>,
    settings: Arc<Mutex<Option<crate::gui::settings::SettingsApp>>>,
    gallery: Arc<Mutex<Option<crate::gui::gallery::GalleryApp>>>,
    toasts: Toasts,
    /// `true` when the viewport uses [`TOAST_WINDOW_SIZE`]; `false` when it is [`TOAST_WINDOW_IDLE_SIZE`].
    overlay_toast_area: bool,
    last_mouse_passthrough: Option<bool>,
    idle_since: Option<std::time::Instant>,
}

impl GuiManagerApp {
    #[cfg(target_os = "windows")]
    fn new(_cc: &eframe::CreationContext<'_>, rx: Receiver<GuiMessage>) -> Self {
        Self {
            rx,
            settings: Arc::new(Mutex::new(None)),
            gallery: Arc::new(Mutex::new(None)),
            toasts: Toasts::default().with_anchor(Anchor::TopRight),
            overlay_toast_area: false,
            last_mouse_passthrough: None,
            idle_since: Some(std::time::Instant::now()),
        }
    }

    #[cfg(not(target_os = "windows"))]
    fn new(cc: &eframe::CreationContext<'_>, rx: Receiver<GuiMessage>) -> Self {
        let mut visuals = cc.egui_ctx.style().visuals.clone();
        visuals.selection.bg_fill = egui::Color32::TRANSPARENT;
        visuals.selection.stroke = egui::Stroke::new(0.0, egui::Color32::TRANSPARENT);
        cc.egui_ctx.set_visuals(visuals);

        Self {
            rx,
            settings: Arc::new(Mutex::new(None)),
            gallery: Arc::new(Mutex::new(None)),
            toasts: Toasts::default().with_anchor(Anchor::TopRight),
            overlay_toast_area: false,
            last_mouse_passthrough: None,
            idle_since: Some(std::time::Instant::now()),
        }
    }

    fn sync_overlay_window_size(&mut self, ctx: &egui::Context) {
        let needs_toast_area = !self.toasts.is_empty();
        if needs_toast_area == self.overlay_toast_area {
            return;
        }
        self.overlay_toast_area = needs_toast_area;

        let size = if needs_toast_area {
            TOAST_WINDOW_SIZE
        } else {
            TOAST_WINDOW_IDLE_SIZE
        };
        let pos = get_toast_window_pos_for_size(size);
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(
            size[0], size[1],
        )));
        ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(
            pos[0], pos[1],
        )));
        ctx.request_repaint();
    }

    fn sync_mouse_passthrough(&mut self, ctx: &egui::Context) {
        // Default to allowing mouse passthrough
        let mut should_passthrough = true;

        // Only consider blocking if we have toasts and mouse is in the viewport
        if !self.toasts.is_empty() {
            if let Some(mouse_pos) = ctx.input(|i| i.pointer.hover_pos()) {
                let viewport_rect = ctx.available_rect();
                if viewport_rect.contains(mouse_pos) {
                    // Only block mouse events in the top-right corner where toasts appear
                    // This leaves most of the overlay area clickable for other windows
                    let toast_region = egui::Rect::from_min_max(
                        egui::pos2(viewport_rect.max.x - 250.0, viewport_rect.min.y),
                        egui::pos2(viewport_rect.max.x, viewport_rect.min.y + 150.0),
                    );

                    should_passthrough = !toast_region.contains(mouse_pos);
                }
            }
        }

        if self.last_mouse_passthrough == Some(should_passthrough) {
            return;
        }
        self.last_mouse_passthrough = Some(should_passthrough);
        ctx.send_viewport_cmd(egui::ViewportCommand::MousePassthrough(should_passthrough));
    }

    fn release_idle_resources(&mut self, ctx: &egui::Context) {
        if !self.toasts.is_empty() {
            return;
        }
        let settings_open = self
            .settings
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some();
        let gallery_open = self
            .gallery
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some();
        if settings_open || gallery_open {
            return;
        }

        self.last_mouse_passthrough = None;
        ctx.memory_mut(|mem| mem.reset_areas());
    }
}

impl eframe::App for GuiManagerApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        egui::Rgba::TRANSPARENT.to_array()
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let mut disconnected = false;
        loop {
            match self.rx.try_recv() {
                Ok(msg) => match msg {
                    GuiMessage::ShowSettings(tx, level_monitor) => {
                        let config = crate::config::Config::load_sync().unwrap_or_default();
                        *self.settings.lock().unwrap_or_else(|e| e.into_inner()) = Some(
                            crate::gui::settings::SettingsApp::new(config, tx, level_monitor),
                        );
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
                                    .closable(false)
                                    .duration(Duration::from_secs(3));
                            }
                            ToastKind::Error => {
                                self.toasts
                                    .error(message)
                                    .closable(false)
                                    .duration(Duration::from_secs(5));
                            }
                            ToastKind::Info => {
                                self.toasts
                                    .info(message)
                                    .closable(false)
                                    .duration(Duration::from_secs(3));
                            }
                            ToastKind::Warning => {
                                self.toasts
                                    .warning(message)
                                    .closable(false)
                                    .duration(Duration::from_secs(4));
                            }
                        }
                        ctx.request_repaint();
                    }
                },
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    disconnected = true;
                    break;
                }
            }
        }

        let frame = egui::Frame::NONE.fill(egui::Color32::TRANSPARENT);
        egui::CentralPanel::default()
            .frame(frame)
            .show(ctx, |_ui| {});

        let settings_clone = self.settings.clone();
        let show_settings = settings_clone
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some();
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
                            settings.release_resources();
                            *lock = None;
                        }
                    }
                },
            );
        }

        let gallery_clone = self.gallery.clone();
        let show_gallery = gallery_clone
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some();
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
                            gallery.release_all_gui_resources();
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

        self.toasts.show(ctx);
        self.sync_mouse_passthrough(ctx);
        self.sync_overlay_window_size(ctx);
        self.release_idle_resources(ctx);

        let settings_open = self
            .settings
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some();
        let gallery_open = self
            .gallery
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some();

        let is_idle = !settings_open && !gallery_open && self.toasts.is_empty();

        if is_idle && self.idle_since.is_none() {
            self.idle_since = Some(std::time::Instant::now());
        } else if !is_idle {
            self.idle_since = None;
        }

        if disconnected {
            with_gui_state(|state| state.request_shutdown());
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}
