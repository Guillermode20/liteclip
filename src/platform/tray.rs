//! System Tray Integration using `tray-icon`
//!
//! Builds and manages a `TrayIcon` with a `muda` context menu.
//! Events are delivered via `TrayIconEvent::receiver()` and
//! `MenuEvent::receiver()` — call `poll_events` from the eframe
//! `update()` loop to drain them.

use anyhow::{Context, Result};
use crossbeam::channel::Sender;
use tracing::{debug, error, info, trace};
use tray_icon::{
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    TrayIcon, TrayIconBuilder, TrayIconEvent,
};

use super::{AppEvent, TrayEvent};

/// Tray menu item IDs.
const ID_SAVE_CLIP: &str = "tray_save_clip";
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
        let separator = PredefinedMenuItem::separator();
        let item_exit = MenuItem::with_id(ID_EXIT, "Exit", true, None);

        let menu = Menu::new();
        menu.append(&item_save).ok();
        menu.append(&separator).ok();
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
            // Left-click now does nothing - no settings to open
        }

        // Context-menu item selections.
        while let Ok(ev) = MenuEvent::receiver().try_recv() {
            info!("Tray menu selected: {}", ev.id.0);
            let tray_event = match ev.id.0.as_str() {
                ID_SAVE_CLIP => Some(TrayEvent::SaveClip),
                ID_EXIT => Some(TrayEvent::Exit),
                other => {
                    trace!("Unknown menu id: {other}");
                    None
                }
            };
            if let Some(te) = tray_event {
                let _ = self
                    .event_tx
                    .send(AppEvent::Tray(te))
                    .map_err(|e| error!("Menu item send: {e}"));
            }
        }
    }
}

/// Load the tray icon, falling back to a solid-colour square if no file is found.
fn load_tray_icon() -> tray_icon::Icon {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()));

    for name in &["liteclip.ico", "icon.ico", "assets/icon.ico"] {
        let path = exe_dir
            .as_ref()
            .map(|d| d.join(name))
            .unwrap_or_else(|| std::path::PathBuf::from(name));

        if path.exists() {
            if let Ok(img) = image::open(&path) {
                let rgba = img.into_rgba8();
                let (w, h) = (rgba.width(), rgba.height());
                if let Ok(icon) = tray_icon::Icon::from_rgba(rgba.into_raw(), w, h) {
                    debug!("Tray: loaded icon from {:?}", path);
                    return icon;
                }
            }
        }
    }

    // Fallback: solid dodger-blue 32 × 32 square.
    let size = 32usize;
    let mut rgba = Vec::with_capacity(size * size * 4);
    for _ in 0..(size * size) {
        rgba.extend_from_slice(&[0x1E, 0x90, 0xFF, 0xFF]);
    }
    tray_icon::Icon::from_rgba(rgba, size as u32, size as u32)
        .expect("fallback tray icon is always valid")
}
