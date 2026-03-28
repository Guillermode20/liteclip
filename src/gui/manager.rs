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
    ShowSettings(
        TokioSender<AppEvent>,
        Option<AudioLevelMonitor>,
        crate::config::Config,
    ),
    ShowGallery(TokioSender<AppEvent>, crate::config::Config),
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
    /// Enables instant wake-on-message without periodic polling.
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
/// Allows toast fade-out animations to complete smoothly.
/// After this period, event loop enters true dormant state (no repaints).
const DORMANT_GRACE_PERIOD_MS: u64 = 1000;
/// GUI Manager for the application.
///
/// Handles centralized display of toasts and manages Settings/Gallery windows.
///
/// # Architecture
///
/// Uses the **dormant EventLoop pattern**: one thread runs for the entire app lifetime.
/// The EventLoop never dies - it just becomes dormant (ControlFlow::Wait) when idle.
/// Windows (Settings/Gallery) are created/destroyed inside `Option` fields, which is
/// fully supported by winit. This avoids the forbidden EventLoop recreation that
/// would cause `RecreationAttempt` panic.
///
/// # Threading
///
/// - GUI thread: Dormant when idle, wakes instantly via EventLoopProxy on messages
/// - Settings/Gallery: Viewports created within the same event loop (deferred viewports)
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
    fn ensure_running(&mut self) {
        if self.thread.is_none() {
            spawn_gui_manager_thread(self);
        }
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

#[derive(Clone, Copy, Debug)]
struct GuiActivityState {
    settings_open: bool,
    gallery_open: bool,
    has_toasts: bool,
}

impl GuiActivityState {
    fn is_truly_idle(self) -> bool {
        !self.settings_open && !self.gallery_open && !self.has_toasts
    }
}

fn should_request_repaint(
    activity: GuiActivityState,
    idle_since: Option<Instant>,
    now: Instant,
) -> bool {
    if activity.has_toasts {
        return true;
    }

    if !activity.is_truly_idle() {
        return false;
    }

    match idle_since {
        Some(idle_since) => {
            now.duration_since(idle_since) < Duration::from_millis(DORMANT_GRACE_PERIOD_MS)
        }
        None => true,
    }
}

pub fn send_gui_message(msg: GuiMessage) {
    init_gui_manager();

    let tx = with_gui_state(|state| state.tx.as_ref().cloned());
    if let Some(tx) = tx {
        if tx.send(msg).is_err() {
            warn!("GUI manager channel closed - message dropped");
        } else {
            // Wake the dormant event loop instantly via proxy.
            // The GUI thread is dormant (ControlFlow::Wait) and will wake
            // immediately to process this message without polling.
            with_gui_state(|state| {
                if let Some(proxy) = &state.event_loop_proxy {
                    let _ = proxy.send_event(UserEvent::RequestRepaint {
                        viewport_id: ViewportId::ROOT,
                        when: Instant::now(),
                        cumulative_pass_nr: 0,
                    });
                }
            });
        }
    } else {
        warn!("GUI manager not initialized - message dropped");
    }
}

pub fn show_toast(kind: ToastKind, message: impl Into<String>) {
    send_gui_message(GuiMessage::Toast(kind, message.into()));
}

pub fn shutdown_gui() {
    // Signal the GUI thread to close by dropping the sender.
    // The GUI thread will detect Disconnected and close gracefully.
    // Note: The thread continues running in dormant state until app exit.
    // The EventLoop is never recreated - it just waits forever after closing windows.
    with_gui_state(|state| {
        state.tx = None;
        state.event_loop_proxy = None;
    });
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

    fn activity_state(&self) -> GuiActivityState {
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

        GuiActivityState {
            settings_open,
            gallery_open,
            has_toasts: !self.toasts.is_empty(),
        }
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
                    GuiMessage::ShowSettings(tx, level_monitor, config) => {
                        *self.settings.lock().unwrap_or_else(|e| e.into_inner()) = Some(
                            crate::gui::settings::SettingsApp::new(config, tx, level_monitor),
                        );
                    }
                    GuiMessage::ShowGallery(tx, config) => {
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

        let now = Instant::now();
        let activity_state = self.activity_state();

        if should_request_repaint(activity_state, self.idle_since, now) {
            ctx.request_repaint_after(Duration::from_millis(ACTIVE_REPAINT_MS));
        }

        self.toasts.show(ctx);
        self.sync_mouse_passthrough(ctx);
        self.sync_overlay_window_size(ctx);
        self.release_idle_resources(ctx);

        let activity_state = self.activity_state();

        if activity_state.is_truly_idle() {
            self.idle_since.get_or_insert(now);
        } else {
            self.idle_since = None;
        }

        if disconnected {
            // Channel closed - close the overlay window but keep event loop alive.
            // The thread will remain dormant until app termination.
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
        let is_idle = GuiActivityState {
            settings_open: false,
            gallery_open: false,
            has_toasts: false,
        }
        .is_truly_idle();
        assert!(
            is_idle,
            "Should be idle when all windows closed and no toasts"
        );

        let is_idle = GuiActivityState {
            settings_open: true,
            gallery_open: false,
            has_toasts: false,
        }
        .is_truly_idle();
        assert!(!is_idle, "Should not be idle when settings is open");

        let is_idle = GuiActivityState {
            settings_open: false,
            gallery_open: true,
            has_toasts: false,
        }
        .is_truly_idle();
        assert!(!is_idle, "Should not be idle when gallery is open");

        let is_idle = GuiActivityState {
            settings_open: false,
            gallery_open: false,
            has_toasts: true,
        }
        .is_truly_idle();
        assert!(!is_idle, "Should not be idle when toasts are visible");

        let is_idle = GuiActivityState {
            settings_open: true,
            gallery_open: true,
            has_toasts: true,
        }
        .is_truly_idle();
        assert!(!is_idle, "Should not be idle when multiple windows open");
    }

    #[test]
    fn test_hidden_windows_do_not_count_as_visible_activity() {
        let activity = GuiActivityState {
            settings_open: true,
            gallery_open: false,
            has_toasts: false,
        };

        assert!(
            !activity.is_truly_idle(),
            "Open-but-hidden window should preserve state and avoid full shutdown"
        );
        assert!(
            !should_request_repaint(activity, None, Instant::now()),
            "Open-but-hidden windows should let the GUI thread park without repaint polling"
        );
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
    /// - Child windows do not force root repaint polling by themselves
    /// - Request periodic repaint when toasts are visible (for animation)
    /// - Request periodic repaint during grace period after becoming idle
    /// - Stop repaint requests when truly idle after grace period expires
    #[test]
    fn test_conditional_repaint_logic() {
        let now = Instant::now();

        let visible_settings_do_not_force_root_repaint = should_request_repaint(
            GuiActivityState {
                settings_open: true,
                gallery_open: false,
                has_toasts: false,
            },
            None,
            now,
        );
        assert!(
            !visible_settings_do_not_force_root_repaint,
            "Visible settings should repaint via their own viewport, not root polling"
        );

        let visible_gallery_do_not_force_root_repaint = should_request_repaint(
            GuiActivityState {
                settings_open: false,
                gallery_open: true,
                has_toasts: false,
            },
            None,
            now,
        );
        assert!(
            !visible_gallery_do_not_force_root_repaint,
            "Visible gallery should repaint via its own viewport, not root polling"
        );

        let should_periodic_for_toasts = should_request_repaint(
            GuiActivityState {
                settings_open: false,
                gallery_open: false,
                has_toasts: true,
            },
            Some(now),
            now,
        );
        assert!(
            should_periodic_for_toasts,
            "Should request periodic repaint when toasts visible"
        );

        let idle_since = Some(now - Duration::from_millis(DORMANT_GRACE_PERIOD_MS / 2));
        let within_grace_period = should_request_repaint(
            GuiActivityState {
                settings_open: false,
                gallery_open: false,
                has_toasts: false,
            },
            idle_since,
            now,
        );
        assert!(
            within_grace_period,
            "Should keep repainting briefly after becoming idle"
        );

        let dormant_idle_since = Some(now - Duration::from_millis(DORMANT_GRACE_PERIOD_MS + 1));
        let should_stay_dormant = !should_request_repaint(
            GuiActivityState {
                settings_open: false,
                gallery_open: false,
                has_toasts: false,
            },
            dormant_idle_since,
            now,
        );
        assert!(
            should_stay_dormant,
            "Should stop repainting once the idle grace period expires"
        );
    }

    #[test]
    fn test_idle_does_not_request_gui_manager_shutdown() {
        let now = Instant::now();

        let open_but_hidden = GuiActivityState {
            settings_open: true,
            gallery_open: false,
            has_toasts: false,
        };
        assert!(
            !open_but_hidden.is_truly_idle(),
            "Open-but-hidden windows should preserve GUI manager state"
        );

        let truly_idle = GuiActivityState {
            settings_open: false,
            gallery_open: false,
            has_toasts: false,
        };
        assert!(
            truly_idle.is_truly_idle(),
            "No windows or toasts should be considered truly idle"
        );
        assert!(
            !should_request_repaint(
                truly_idle,
                Some(now - Duration::from_millis(DORMANT_GRACE_PERIOD_MS + 1)),
                now,
            ),
            "A truly idle GUI manager should go dormant without destroying the event loop"
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
