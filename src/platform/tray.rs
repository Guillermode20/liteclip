//! System Tray Integration using `tray-icon`
//!
//! Builds and manages a `TrayIcon` with a `muda` context menu.
//! Events are delivered via `TrayIconEvent::receiver()` and
//! `MenuEvent::receiver()` — call `poll_events` from the eframe
//! `update()` loop to drain them.

use anyhow::{Context, Result};
use crossbeam::channel::Sender;
use tracing::warn;
use tracing::{debug, error, info, trace};
use tray_icon::{
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    TrayIcon, TrayIconBuilder, TrayIconEvent,
};

use super::{AppEvent, TrayEvent};

/// Tray menu item IDs.
const ID_SAVE_CLIP: &str = "tray_save_clip";
const ID_OPEN_SETTINGS: &str = "tray_open_settings";
const ID_OPEN_GALLERY: &str = "tray_open_gallery";
const ID_RESTART: &str = "tray_restart";
const ID_EXIT: &str = "tray_exit";

/// Managed system-tray icon.
///
/// Drop to remove the icon.  Call [`TrayManager::poll_events`] regularly
/// from the GUI thread to forward tray interactions to the main event bus.
pub struct TrayManager {
    /// Live tray icon (dropping removes it from the notification area).
    _icon: TrayIcon,
    /// Channel to the main async event loop.
    event_tx: Sender<AppEvent>,
}

impl TrayManager {
    /// Create the tray icon and its context menu.
    pub fn new(event_tx: Sender<AppEvent>) -> Result<Self> {
        debug!("Initialising tray-icon");

        let item_save = MenuItem::with_id(ID_SAVE_CLIP, "Save Clip", true, None);
        let item_settings = MenuItem::with_id(ID_OPEN_SETTINGS, "Open Settings", true, None);
        let item_gallery = MenuItem::with_id(ID_OPEN_GALLERY, "Open Gallery", true, None);
        let separator1 = PredefinedMenuItem::separator();
        let item_restart = MenuItem::with_id(ID_RESTART, "Restart", true, None);
        let item_exit = MenuItem::with_id(ID_EXIT, "Exit", true, None);

        let menu = Menu::new();
        menu.append(&item_save).ok();
        menu.append(&item_settings).ok();
        menu.append(&item_gallery).ok();
        menu.append(&separator1).ok();
        menu.append(&item_restart).ok();
        menu.append(&item_exit).ok();

        let icon = load_tray_icon();

        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .with_tooltip("LiteClip Replay")
            .with_icon(icon)
            .build()
            .context("Failed to build tray icon")?;

        info!("System tray icon created");

        Ok(Self {
            _icon: tray,
            event_tx,
        })
    }

    /// Drain pending tray + menu events and forward them as [`AppEvent`]s.
    ///
    /// Must be called from the **GUI / event-loop thread**.
    pub fn poll_events(&self) {
        // Left/right-click on the notification-area icon itself.
        while let Ok(ev) = TrayIconEvent::receiver().try_recv() {
            trace!("Tray-icon event received: {:?}", ev);
        }

        // Context-menu item selections - drain ALL pending events
        loop {
            match MenuEvent::receiver().try_recv() {
                Ok(ev) => {
                    info!("Tray menu selected: {}", ev.id.0);
                    let tray_event = match ev.id.0.as_str() {
                        ID_SAVE_CLIP => Some(TrayEvent::SaveClip),
                        ID_OPEN_SETTINGS => Some(TrayEvent::OpenSettings),
                        ID_OPEN_GALLERY => Some(TrayEvent::OpenGallery),
                        ID_RESTART => Some(TrayEvent::Restart),
                        ID_EXIT => {
                            info!("Exit menu item clicked - sending Exit event");
                            Some(TrayEvent::Exit)
                        }
                        other => {
                            trace!("Unknown menu id: {other}");
                            None
                        }
                    };
                    if let Some(te) = tray_event {
                        if let Err(e) = self.event_tx.send(AppEvent::Tray(te)) {
                            error!("Failed to send tray event: {e}");
                        }
                    }
                }
                Err(crossbeam::channel::TryRecvError::Empty) => break,
                Err(crossbeam::channel::TryRecvError::Disconnected) => {
                    warn!("Menu event channel disconnected");
                    break;
                }
            }
        }
    }
}

/// Load the tray icon, falling back to a solid-colour square if no file is found.
fn load_tray_icon() -> tray_icon::Icon {
    let icon_data = include_bytes!("../../logo.ico");
    if let Ok(img) = image::load_from_memory(icon_data) {
        let rgba = img.into_rgba8();
        let (w, h) = (rgba.width(), rgba.height());
        if let Ok(icon) = tray_icon::Icon::from_rgba(rgba.into_raw(), w, h) {
            debug!("Tray: loaded baked icon");
            return icon;
        }
    }

    // Fallback: solid dodger-blue 16 × 16 square (minimum recommended size for tray icons)
    // Using smaller size to avoid potential icon dimension issues
    let size = 16usize;
    let mut rgba = Vec::with_capacity(size * size * 4);
    for _ in 0..(size * size) {
        rgba.extend_from_slice(&[0x1E, 0x90, 0xFF, 0xFF]);
    }
    match tray_icon::Icon::from_rgba(rgba, size as u32, size as u32) {
        Ok(icon) => {
            debug!("Tray: using fallback icon");
            icon
        }
        Err(e) => {
            warn!(
                "Failed to create fallback icon: {}. Using default system icon.",
                e
            );
            // Return a transparent 1x1 icon as last resort - tray-icon crate will handle this
            let transparent = vec![0, 0, 0, 0];
            tray_icon::Icon::from_rgba(transparent, 1, 1)
                .expect("1x1 transparent icon must be valid")
        }
    }
}
