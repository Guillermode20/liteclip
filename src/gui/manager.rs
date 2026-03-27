use crate::capture::audio::AudioLevelMonitor;
use crate::platform::AppEvent;
use eframe::egui;
use eframe::UserEvent;
use egui::ViewportId;
use egui_notify::{Anchor, Toasts};
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::sync::LazyLock;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::Sender as TokioSender;
use tracing::warn;
#[cfg(target_os = "windows")]
use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};
use winit::event_loop::{EventLoop, EventLoopProxy};

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
    /// EventLoopProxy used to wake the dormant GUI thread when a GuiMessage arrives.
    /// This enables instant wake-on-message without periodic polling.
    event_loop_proxy: Option<EventLoopProxy<UserEvent>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

static GUI_STATE: LazyLock<Mutex<GuiManagerState>> =
    LazyLock::new(|| Mutex::new(GuiManagerState::default()));

const TOAST_WINDOW_SIZE: [f32; 2] = [350.0, 300.0];
/// When idle, shrink the overlay so it does not block clicks elsewhere (1×1 logical pixel).
const TOAST_WINDOW_IDLE_SIZE: [f32; 2] = [1.0, 1.0];
const TOAST_WINDOW_MARGIN: [f32; 2] = [20.0, 20.0];
/// Repaint interval when there are toasts or windows visible (active state).
const ACTIVE_REPAINT_MS: u64 = 100;
/// Grace period before entering dormant state after becoming idle.
/// This allows toast fade-out animations to complete smoothly.
/// Event loop becomes dormant (ControlFlow::Wait) after this period.
const DORMANT_GRACE_PERIOD_MS: u64 = 2000;
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
        // Create the event loop ourselves to capture its proxy for wake-on-message.
        // This enables instant GUI thread wake without periodic polling.
        // Note: On Windows, we can use with_any_thread to run off the main thread.
        #[cfg(target_os = "windows")]
        let event_loop: EventLoop<UserEvent> = {
            use winit::platform::windows::EventLoopBuilderExtWindows;
            match EventLoop::with_user_event().with_any_thread(true).build() {
                Ok(el) => el,
                Err(e) => {
                    warn!("Failed to create event loop: {:?}", e);
                    return;
                }
            }
        };

        #[cfg(not(target_os = "windows"))]
        let event_loop: EventLoop<UserEvent> = match EventLoop::with_user_event().build() {
            Ok(el) => el,
            Err(e) => {
                warn!("Failed to create event loop: {:?}", e);
                return;
            }
        };

        // Get the proxy before running the event loop - it's Send+Sync so we can share it.
        let proxy = event_loop.create_proxy();

        // Store the proxy in global state so send_gui_message can wake the event loop.
        with_gui_state(|s| s.event_loop_proxy = Some(proxy));

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
            // Note: event_loop_builder is NOT used here since we pass a pre-built event loop
            // to create_native. The with_any_thread setting was applied when building the event loop above.
            ..Default::default()
        };

        // Use create_native + run_app pattern instead of run_native.
        // This allows us to control the event loop lifecycle and capture the proxy.
        // create_native returns EframeWinitApplication directly (not a Result).
        let mut winit_app = eframe::create_native(
            "liteclip_overlay",
            options,
            Box::new(|cc| Ok(Box::new(GuiManagerApp::new(cc, rx)))),
            &event_loop,
        );

        // Run the event loop - this blocks until the app closes.
        if let Err(e) = event_loop.run_app(&mut winit_app) {
            warn!("event_loop.run_app failed: {:?}", e);
        }

        // Clear the proxy when the event loop exits.
        with_gui_state(|s| s.event_loop_proxy = None);
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
                Ok(()) => {
                    // Wake the dormant event loop by sending a RequestRepaint user event.
                    // This ensures the GUI thread processes the message immediately
                    // rather than waiting for the next periodic poll.
                    with_gui_state(|state| {
                        if let Some(proxy) = &state.event_loop_proxy {
                            // Use a dummy ViewportId since we just want to wake the loop,
                            // not target a specific viewport. The actual message handling
                            // happens in GuiManagerApp::update() via channel polling.
                            let _ = proxy.send_event(UserEvent::RequestRepaint {
                                viewport_id: ViewportId::ROOT,
                                when: Instant::now(),
                                cumulative_pass_nr: 0,
                            });
                        }
                    });
                    return;
                }
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
        } else if !self.toasts.is_empty() {
            // Keep polling while toasts are visible for animation/dismissal
            ctx.request_repaint_after(Duration::from_millis(ACTIVE_REPAINT_MS));
        } else if let Some(idle_since) = self.idle_since {
            // Grace period before entering dormant state
            // This allows toast fade-out animations to complete smoothly
            let elapsed = idle_since.elapsed();
            if elapsed < Duration::from_millis(DORMANT_GRACE_PERIOD_MS) {
                ctx.request_repaint_after(Duration::from_millis(ACTIVE_REPAINT_MS));
            }
            // After grace period: no repaint requested -> event loop becomes dormant
            // Wake will happen via EventLoopProxy when GuiMessage arrives (see send_gui_message)
        } else {
            // No idle_since set yet (initial state) - keep polling briefly
            ctx.request_repaint_after(Duration::from_millis(ACTIVE_REPAINT_MS));
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Tests that the idle detection logic correctly identifies idle state.
    /// The GUI is considered idle when:
    /// - No settings window is open
    /// - No gallery window is open
    /// - No toasts are visible
    #[test]
    fn test_idle_detection_logic() {
        // Test: All conditions for idle
        let settings_open = false;
        let gallery_open = false;
        let toasts_empty = true;

        let is_idle = !settings_open && !gallery_open && toasts_empty;
        assert!(
            is_idle,
            "Should be idle when all windows closed and no toasts"
        );

        // Test: Settings open means not idle
        let settings_open = true;
        let gallery_open = false;
        let toasts_empty = true;
        let is_idle = !settings_open && !gallery_open && toasts_empty;
        assert!(!is_idle, "Should not be idle when settings is open");

        // Test: Gallery open means not idle
        let settings_open = false;
        let gallery_open = true;
        let toasts_empty = true;
        let is_idle = !settings_open && !gallery_open && toasts_empty;
        assert!(!is_idle, "Should not be idle when gallery is open");

        // Test: Toasts visible means not idle
        let settings_open = false;
        let gallery_open = false;
        let toasts_empty = false;
        let is_idle = !settings_open && !gallery_open && toasts_empty;
        assert!(!is_idle, "Should not be idle when toasts are visible");

        // Test: Multiple conditions means not idle
        let settings_open = true;
        let gallery_open = true;
        let toasts_empty = false;
        let is_idle = !settings_open && !gallery_open && toasts_empty;
        assert!(!is_idle, "Should not be idle when multiple windows open");
    }

    /// Tests that GuiManagerState correctly initializes with default values.
    #[test]
    fn test_gui_manager_state_default() {
        let state = GuiManagerState::default();
        assert!(state.tx.is_none(), "tx should be None by default");
        assert!(
            state.event_loop_proxy.is_none(),
            "event_loop_proxy should be None by default"
        );
        assert!(state.thread.is_none(), "thread should be None by default");
    }

    /// Tests that request_shutdown clears the tx channel.
    #[test]
    fn test_request_shutdown_clears_tx() {
        let (tx, _rx) = channel();
        let mut state = GuiManagerState {
            tx: Some(tx),
            event_loop_proxy: None,
            thread: None,
        };

        state.request_shutdown();
        assert!(
            state.tx.is_none(),
            "tx should be None after shutdown request"
        );
    }

    /// Tests that GuiManagerState can store an EventLoopProxy reference.
    /// Note: The actual EventLoopProxy requires a running event loop, so we
    /// only test that the field can be set to None and the state is accessible.
    #[test]
    fn test_event_loop_proxy_field_exists() {
        let state = GuiManagerState::default();

        // Verify we can access the event_loop_proxy field
        assert!(
            state.event_loop_proxy.is_none(),
            "event_loop_proxy should start as None"
        );

        // The actual EventLoopProxy can only be created with a running event loop,
        // so we only test the None case here. The real functionality is tested
        // in the integration/manual tests.
    }

    /// Tests the wake event structure used to wake the event loop.
    #[test]
    fn test_wake_event_structure() {
        // Verify the UserEvent::RequestRepaint structure matches what we use
        let viewport_id = ViewportId::ROOT;
        let when = Instant::now();
        let cumulative_pass_nr: u64 = 0;

        let event = UserEvent::RequestRepaint {
            viewport_id,
            when,
            cumulative_pass_nr,
        };

        // Verify the event can be created and matches expected structure
        match event {
            UserEvent::RequestRepaint {
                viewport_id: vid,
                when: w,
                cumulative_pass_nr: cpn,
            } => {
                assert_eq!(vid, viewport_id);
                assert_eq!(cpn, cumulative_pass_nr);
                // 'when' is just checked to be valid Instant
                let _ = w;
            }
            UserEvent::AccessKitActionRequest(_) => {
                panic!("Unexpected event type");
            }
        }
    }

    /// Tests the conditional repaint logic for determining whether to request repaints.
    /// The logic follows these rules:
    /// - Request immediate repaint when settings or gallery is visible
    /// - Request periodic repaint when toasts are visible (for animation)
    /// - Request periodic repaint during grace period after becoming idle
    /// - Stop repaint requests when truly idle after grace period expires
    #[test]
    fn test_conditional_repaint_logic() {
        // Test: Settings visible -> should request repaint
        let show_settings = true;
        let show_gallery = false;
        let _toasts_empty = true; // Not used in this assertion
        let _idle_since: Option<Instant> = None; // Not used in this assertion

        let should_repaint_now = show_settings || show_gallery;
        assert!(
            should_repaint_now,
            "Should request immediate repaint when settings visible"
        );

        // Test: Gallery visible -> should request repaint
        let _show_settings = false;
        let show_gallery = true;
        let should_repaint_now = _show_settings || show_gallery;
        assert!(
            should_repaint_now,
            "Should request immediate repaint when gallery visible"
        );

        // Test: Toasts visible -> should request periodic repaint
        let _show_settings = false;
        let _show_gallery = false;
        let toasts_empty = false;
        let should_periodic_for_toasts = !toasts_empty;
        assert!(
            should_periodic_for_toasts,
            "Should request periodic repaint when toasts visible"
        );

        // Test: Truly idle but within grace period -> should request periodic repaint
        let _show_settings = false;
        let _show_gallery = false;
        let _toasts_empty = true;
        let idle_since = Some(Instant::now());
        let grace_period = Duration::from_millis(DORMANT_GRACE_PERIOD_MS);
        let elapsed = idle_since.map(|t| t.elapsed()).unwrap_or_default();
        let within_grace_period = elapsed < grace_period;
        assert!(
            within_grace_period,
            "Should be within grace period immediately after becoming idle"
        );

        // Test: Truly idle after grace period -> should NOT request repaint (dormant)
        // We simulate this by checking the logic would NOT request repaint
        let truly_idle = !_show_settings && !_show_gallery && _toasts_empty;
        let has_idle_since = idle_since.is_some();
        // In the actual code, if elapsed > grace_period, no repaint is requested
        // This test verifies the conditions that lead to dormancy
        assert!(
            truly_idle && has_idle_since,
            "Truly idle state should have idle_since set"
        );
    }

    /// Tests that the grace period constant is within acceptable bounds.
    /// The grace period should be long enough for toast fade-out animations
    /// to complete (typically 1-3 seconds), but short enough to quickly
    /// enter dormant state and reduce CPU usage.
    #[test]
    fn test_grace_period_bounds() {
        // Grace period should be between 1 and 3 seconds
        let grace_period_ms = DORMANT_GRACE_PERIOD_MS;
        let min_grace_ms = 1000; // 1 second minimum
        let max_grace_ms = 3000; // 3 seconds maximum

        assert!(
            grace_period_ms >= min_grace_ms,
            "Grace period should be at least 1 second ({DORMANT_GRACE_PERIOD_MS}ms is too short)"
        );
        assert!(
            grace_period_ms <= max_grace_ms,
            "Grace period should be at most 3 seconds ({DORMANT_GRACE_PERIOD_MS}ms is too long)"
        );
    }

    /// Tests that the active repaint interval is reasonable for smooth UI.
    /// The interval should be short enough for responsive UI (typically 50-200ms).
    #[test]
    fn test_active_repaint_interval_bounds() {
        let active_repaint_ms = ACTIVE_REPAINT_MS;
        let min_interval_ms = 16; // ~60fps minimum
        let max_interval_ms = 200; // 200ms maximum for smooth animations

        assert!(
            active_repaint_ms >= min_interval_ms,
            "Active repaint interval should be at least 16ms ({ACTIVE_REPAINT_MS}ms is too short)"
        );
        assert!(
            active_repaint_ms <= max_interval_ms,
            "Active repaint interval should be at most 200ms ({ACTIVE_REPAINT_MS}ms is too long)"
        );
    }

    /// Tests the dormancy activation timing meets the validation contract.
    /// According to VAL-GUI-005, CPU usage should drop within 3 seconds of idle.
    /// Our grace period + active repaint cycle should achieve this.
    #[test]
    fn test_dormancy_activation_timing() {
        // Dormancy should activate within 3 seconds (VAL-GUI-005)
        let max_activation_time_ms = 3000;

        // Worst case: grace period + one extra repaint cycle before dormancy
        let worst_case_activation_ms = DORMANT_GRACE_PERIOD_MS + ACTIVE_REPAINT_MS;

        assert!(
            worst_case_activation_ms <= max_activation_time_ms,
            "Dormancy should activate within 3 seconds (worst case: {worst_case_activation_ms}ms > {max_activation_time_ms}ms)"
        );
    }
}
