# GUI Thread CPU Usage Analysis

## Executive Summary

**The primary source of idle CPU usage is `manager.rs` line 432**, which schedules a repaint every 100ms even when no windows are visible. This keeps the eframe/winit event loop in a polling state rather than dormant.

**Estimated idle CPU impact**: ~0.5-2% CPU (varies by system) from:
- 10 repaints per second on a 1x1 pixel window
- Event loop polling at 100ms interval
- egui context state management

---

## Current CPU Usage Sources (When "Idle")

### 1. Primary Source: Overlay Window Periodic Repaint

**File**: `src/gui/manager.rs`
**Location**: Lines 428-433

```rust
if show_settings || show_gallery {
    ctx.request_repaint();
} else {
    ctx.request_repaint_after(Duration::from_millis(IDLE_REPAINT_MS));  // IDLE_REPAINT_MS = 100
}
```

**Impact**: This is the root cause. Even when:
- No toasts are visible
- Settings window is closed
- Gallery window is closed
- Overlay is shrunk to 1x1 pixel (`TOAST_WINDOW_IDLE_SIZE`)

...the overlay window still repaints every 100ms, keeping the event loop active.

### 2. Channel Polling Loop

**File**: `src/gui/manager.rs`
**Location**: Lines 326-363 (within `update()`)

```rust
loop {
    match self.rx.try_recv() {
        Ok(msg) => { /* handle message */ },
        Err(TryRecvError::Empty) => break,
        Err(TryRecvError::Disconnected) => { /* shutdown */ }
    }
}
```

**Impact**: Minor - `try_recv()` is non-blocking and cheap. However, this runs 10 times per second due to the repaint timer.

### 3. Overlay State Sync Functions

**File**: `src/gui/manager.rs`
**Location**: Lines 233-264, 265-287, 288-304

```rust
fn sync_overlay_window_size(&mut self, ctx: &egui::Context) { ... }
fn sync_mouse_passthrough(&mut self, ctx: &egui::Context) { ... }
fn release_idle_resources(&mut self, ctx: &egui::Context) { ... }
```

**Impact**: These run every repaint (every 100ms when idle), but they do minimal work when truly idle:
- `sync_overlay_window_size`: Checks `toasts.is_empty()`, returns early
- `sync_mouse_passthrough`: Early return when no state change
- `release_idle_resources`: Resets memory areas when idle

### 4. Main Event Loop Timeout Polling

**File**: `src/main.rs`
**Location**: Lines 364-440

```rust
tokio::select! {
    result = tokio::time::timeout(
        tokio::time::Duration::from_millis(100),
        tokio_rx.recv()
    ) => {
        // On timeout: enforce_pipeline_health() is called
    }
}
```

**Impact**: The main event loop polls with a 100ms timeout. When timeout occurs (no platform events), it:
- Calls `enforce_pipeline_health()` to check pipeline status
- Logs memory telemetry every 30 seconds

This is independent of GUI CPU usage but contributes to overall background activity.

### 5. Settings Window Audio Meter (When Open)

**File**: `src/gui/settings.rs`
**Location**: Lines 581-584

```rust
if self.level_monitor.is_some() {
    ctx.request_repaint_after(std::time::Duration::from_millis(33));
}
```

**Impact**: Only when Settings window is open. Causes ~30 fps repaints for audio level meter animation.

### 6. Gallery Editor Playback (When Active)

**File**: `src/gui/gallery.rs`
**Location**: Lines 985-991

```rust
if self.should_repaint() {
    let repaint_ms = self.editor
        .as_ref()
        .filter(|e| e.is_playing)
        .map(|e| (1000.0 / e.playback.playback_fps()).clamp(8.0, 50.0) as u64)
        .unwrap_or(80);
    ctx.request_repaint_after(Duration::from_millis(repaint_ms));
}
```

**Impact**: Only when Gallery/editor is open and active (playing video, exporting).

---

## Code Locations Contributing to Idle CPU

| File | Line(s) | Description | Active When |
|------|---------|-------------|-------------|
| `manager.rs` | 432 | `request_repaint_after(100ms)` | **Always** (root cause) |
| `manager.rs` | 326-363 | Channel polling loop | Every repaint |
| `manager.rs` | 233-304 | State sync functions | Every repaint |
| `main.rs` | 364-440 | 100ms timeout polling | Always (main thread) |
| `settings.rs` | 583 | 33ms repaint for audio meter | Settings open |
| `gallery.rs` | 990 | 8-80ms repaint | Gallery active |
| `editor.rs` | 133 | 16ms for export modal | Exporting |

---

## Changes Needed for "Completely Dormant" State

### Primary Change: Conditional Repaint in Overlay

**Location**: `src/gui/manager.rs` line 430-433

**Current code**:
```rust
if show_settings || show_gallery {
    ctx.request_repaint();
} else {
    ctx.request_repaint_after(Duration::from_millis(IDLE_REPAINT_MS));
}
```

**Proposed change**:
```rust
// Only request repaint when there's actual work to do
if show_settings || show_gallery {
    ctx.request_repaint();
} else if !self.toasts.is_empty() {
    // Keep polling for toast animations/dismissal
    ctx.request_repaint_after(Duration::from_millis(IDLE_REPAINT_MS));
} else if self.idle_since.is_some() {
    // Truly idle - stop all repaint requests
    // Let the event loop become dormant (ControlFlow::Wait)
    // Wake will happen via channel message (GuiMessage)
}
```

### Secondary Changes

1. **Wake on message arrival**: When a `GuiMessage::Toast` arrives, ensure `ctx.request_repaint()` is called to wake the dormant event loop. This is already done at line 351.

2. **Idle threshold**: Optionally, add a threshold before stopping repaints entirely:
   ```rust
   const IDLE_GRACE_PERIOD_MS: u64 = 500; // Wait before becoming dormant
   
   if let Some(idle_since) = self.idle_since {
       if idle_since.elapsed() > Duration::from_millis(IDLE_GRACE_PERIOD_MS) {
           // Stop repaints - event loop dormant
       } else {
           ctx.request_repaint_after(Duration::from_millis(IDLE_REPAINT_MS));
       }
   }
   ```

3. **Alternative: Use `ControlFlow::Wait` directly**: Per `eframe-on-demand.md`, the event loop naturally becomes dormant when no repaints are requested. The above change should achieve this without needing `create_native` + manual pumping.

---

## Risk Assessment

### High Risk Areas

| Risk | Description | Mitigation |
|------|-------------|------------|
| **Toast display timing** | First toast might not appear immediately if event loop is dormant | The `send_gui_message` path calls `ctx.request_repaint()` at line 351 when receiving a Toast message |
| **Toast dismissal animation** | Toasts have 3-5 second duration with fade-out animation | Need to keep event loop active while `toasts.is_empty() == false` |
| **Channel polling** | Messages might be delayed if event loop is dormant | The channel uses `std::sync::mpsc` which wakes the receiving thread; however, winit's event loop may not wake on channel activity |

### Medium Risk Areas

| Risk | Description | Mitigation |
|------|-------------|------------|
| **Mouse passthrough sync** | `sync_mouse_passthrough` won't run when dormant | Only matters when toasts are visible, which keeps event loop active |
| **Window size sync** | `sync_overlay_window_size` won't run when dormant | Only matters when toasts appear/disappear, which triggers repaint |

### Low Risk Areas

| Risk | Description | Mitigation |
|------|-------------|------------|
| **Settings/Gallery opening** | Need to wake event loop when opening | Already handled: `GuiMessage::ShowSettings/ShowGallery` triggers `ctx.request_repaint()` |
| **Memory cleanup** | `release_idle_resources` won't run when dormant | Safe: resources are released when windows close, not during idle |

---

## Critical Channel Wake Mechanism

The overlay GUI thread receives messages via `std::sync::mpsc::Receiver`. When the event loop is dormant (no repaint scheduled), **how does it wake on channel activity?**

**Current behavior**: The 100ms `request_repaint_after` wakes the event loop, which then polls the channel via `try_recv()`.

**Risk if dormant**: The `std::sync::mpsc::Receiver` does NOT wake a winit event loop. The sender posts to the channel, but the receiver thread (GUI thread) is blocked in winit's event loop waiting for OS events.

**Solution options**:

1. **Use winit's `EventLoopProxy`**: The `eframe::NativeOptions` builder allows setting up a user event channel. Send a wake event via `EventLoopProxy::send_event()` when a `GuiMessage` arrives.

2. **Keep minimal polling**: Instead of stopping all repaints, reduce to a longer interval (e.g., 1 second) to periodically check the channel while still being mostly dormant.

3. **Hybrid approach**: Use `request_repaint_after(Duration::MAX)` which effectively tells egui "no repaint needed until explicitly requested". Then use a separate mechanism to wake on channel activity.

### Recommended Approach

Modify the `GuiManagerState` to use winit's `EventLoopProxy` for wake-on-message:

```rust
// In spawn_gui_manager_thread:
let event_loop_proxy = ...; // Get from eframe NativeOptions

// When sending message:
pub fn send_gui_message(msg: GuiMessage) {
    // Send to channel
    tx.send(msg);
    // Wake the event loop
    event_loop_proxy.send_event(WakeEvent);
}
```

This is more invasive but achieves true dormant state with instant wake on activity.

---

## Alternative: Minimal Polling Approach (Lower Risk)

For a safer, simpler change without winit integration:

```rust
const IDLE_POLL_INTERVAL_MS: u64 = 1000; // 1 second when truly idle

if show_settings || show_gallery {
    ctx.request_repaint();
} else if !self.toasts.is_empty() {
    ctx.request_repaint_after(Duration::from_millis(100)); // Toast animations
} else {
    // Truly idle - poll very slowly
    ctx.request_repaint_after(Duration::from_millis(IDLE_POLL_INTERVAL_MS));
}
```

**Impact**: Reduces idle CPU from 10 repaints/sec to 1 repaint/sec (~90% reduction) without risking missed messages.

---

## Summary of Recommended Changes

| Priority | Change | Risk | CPU Impact |
|----------|--------|------|------------|
| **High** | Remove `request_repaint_after(100ms)` when truly idle | Medium | ~0.5-2% CPU saved |
| **Medium** | Add winit `EventLoopProxy` wake mechanism | Medium | Enables true dormant |
| **Low** | Increase idle poll to 1 second | Low | ~90% reduction, safe |

**Recommended order**:
1. Start with the **Low** risk change (1 second polling) to verify behavior
2. Then implement the **High** change with proper wake mechanism
3. Consider the **Medium** winit integration if needed

---

## Verification Steps

After implementing changes:

1. **Monitor CPU**: Use Windows Task Manager or `Process Explorer` to measure idle CPU
2. **Test toast timing**: Send toast via tray menu, verify it appears within 100ms
3. **Test settings opening**: Open settings from tray, verify instant response
4. **Test gallery opening**: Open gallery via hotkey/tray, verify instant response
5. **Long idle test**: Run app for 30 minutes idle, verify CPU remains ~0%
