//! Event simulation utilities for testing user interactions.
//!
//! Provides mechanisms to simulate hotkey presses and tray menu interactions
//! without requiring actual Windows messages or GUI automation.

use liteclip::platform::{HotkeyAction, TrayEvent};
use std::sync::mpsc::{channel, Receiver, Sender};

/// Simulates platform events (hotkeys, tray clicks) for testing.
///
/// This is a test double that replaces the actual platform event loop,
/// allowing tests to inject events programmatically.
pub struct EventSimulator {
    hotkey_tx: Sender<HotkeyAction>,
    tray_tx: Sender<TrayEvent>,
}

impl EventSimulator {
    /// Creates a new event simulator with the specified channels.
    pub fn new(hotkey_tx: Sender<HotkeyAction>, tray_tx: Sender<TrayEvent>) -> Self {
        Self { hotkey_tx, tray_tx }
    }

    /// Simulates a hotkey press.
    ///
    /// Sends the hotkey action to the application event loop.
    pub fn press_hotkey(&self, action: HotkeyAction) -> anyhow::Result<()> {
        self.hotkey_tx
            .send(action)
            .map_err(|_| anyhow::anyhow!("Failed to send hotkey event"))?;
        Ok(())
    }

    /// Simulates a tray menu click.
    ///
    /// Sends the tray event to the application event loop.
    pub fn click_tray(&self, event: TrayEvent) -> anyhow::Result<()> {
        self.tray_tx
            .send(event)
            .map_err(|_| anyhow::anyhow!("Failed to send tray event"))?;
        Ok(())
    }
}

/// Creates a paired event simulator and receiver.
///
/// Returns a tuple of (simulator, hotkey_receiver, tray_receiver).
/// The receivers can be used to verify that events were sent.
pub fn create_event_pair() -> (EventSimulator, Receiver<HotkeyAction>, Receiver<TrayEvent>) {
    let (hotkey_tx, hotkey_rx) = channel();
    let (tray_tx, tray_rx) = channel();
    let simulator = EventSimulator::new(hotkey_tx, tray_tx);
    (simulator, hotkey_rx, tray_rx)
}

/// Mock platform handle for testing without actual Windows APIs.
///
/// Records events for verification rather than interacting with Windows.
pub struct MockPlatformHandle {
    events: Vec<PlatformEvent>,
}

#[derive(Debug, Clone)]
pub enum PlatformEvent {
    HotkeyRegistered { action: HotkeyAction, key: String },
    HotkeyUnregistered { action: HotkeyAction },
    TrayIconUpdated { recording: bool },
    NotificationShown { title: String, message: String },
}

impl MockPlatformHandle {
    /// Creates a new mock platform handle.
    pub fn new() -> Self {
        Self { events: Vec::new() }
    }

    /// Records a hotkey registration.
    pub fn record_hotkey_registered(&mut self, action: HotkeyAction, key: String) {
        self.events
            .push(PlatformEvent::HotkeyRegistered { action, key });
    }

    /// Records a tray update.
    pub fn record_tray_update(&mut self, recording: bool) {
        self.events
            .push(PlatformEvent::TrayIconUpdated { recording });
    }

    /// Returns all recorded events.
    pub fn events(&self) -> &[PlatformEvent] {
        &self.events
    }

    /// Clears all recorded events.
    pub fn clear_events(&mut self) {
        self.events.clear();
    }

    /// Asserts that a specific event was recorded.
    pub fn assert_event_occurred(&self, expected: &PlatformEvent) {
        assert!(
            self.events
                .iter()
                .any(|e| format!("{:?}", e) == format!("{:?}", expected)),
            "Expected event {:?} not found in recorded events: {:?}",
            expected,
            self.events
        );
    }
}

impl Default for MockPlatformHandle {
    fn default() -> Self {
        Self::new()
    }
}
