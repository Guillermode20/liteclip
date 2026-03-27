# Research: eframe `run_app_on_demand` and Event Loop Pause/Resume

## Executive Summary

**eframe 0.33.3 directly supports running on an external event loop** via:
- `create_native()` - creates an `EframeWinitApplication` that can run on your own event loop
- `pump_eframe_app()` - pumps the event loop on demand (similar to winit's `run_app_on_demand`)

This is **NOT** using winit's deprecated `run_on_demand` directly, but provides equivalent functionality through the new `ApplicationHandler` API introduced in winit 0.30.

---

## Current Versions in LiteClip Project

From `Cargo.toml`:
- **eframe**: `0.33.3`
- **egui**: `0.33.3`
- **winit**: `0.30` (direct dependency)

The project already uses compatible versions that support the external event loop feature.

---

## Direct Support: `create_native` + `pump_eframe_app`

### API Overview

eframe 0.33.3 (released with PR #6750) exposes these key APIs:

```rust
// Create an eframe app that runs on your own event loop
pub fn create_native<'a>(
    app_name: &str,
    native_options: NativeOptions,
    app_creator: AppCreator<'a>,
    event_loop: &EventLoop<UserEvent>,
) -> EframeWinitApplication<'a>

// Pump the event loop manually (returns ControlFlow or Exit)
pub fn pump_eframe_app(
    &mut self,
    event_loop: &mut EventLoop<UserEvent>,
    timeout: Option<Duration>,
) -> EframePumpStatus
```

### EframePumpStatus Enum

```rust
pub enum EframePumpStatus {
    Continue(ControlFlow),  // Continue running, check ControlFlow
    Exit(i32),              // Application requested exit
}
```

### EframeWinitApplication

This struct implements `winit::ApplicationHandler<UserEvent>` and can:
- Be run directly via `event_loop.run_app(&mut winit_app)`
- Be pumped manually via `pump_eframe_app()` for integration with other frameworks

---

## Pattern for Dormant GUI Thread (Event Loop Paused)

### Approach 1: Using `ControlFlow::Wait`

When all windows are hidden/closed, set `ControlFlow::Wait` to make the event loop dormant:

```rust
use eframe::{create_native, EframeWinitApplication, UserEvent};
use winit::event_loop::{ControlFlow, EventLoop};

fn main() -> eframe::Result {
    let eventloop = EventLoop::<UserEvent>::with_user_event().build()?;
    eventloop.set_control_flow(ControlFlow::Wait); // Dormant until events arrive

    let mut winit_app = create_native(
        "LiteClip",
        native_options,
        app_creator,
        &eventloop,
    );

    eventloop.run_app(&mut winit_app)?;
    Ok(())
}
```

### Approach 2: Manual Pumping (for Tokio/async integration)

The `pump_eframe_app` method allows integrating eframe with async frameworks:

```rust
use eframe::{create_native, EframePumpStatus, UserEvent};
use winit::event_loop::{ControlFlow, EventLoop};

let mut eventloop = EventLoop::<UserEvent>::with_user_event().build()?;
let mut winit_app = create_native(..., &eventloop);

let mut control_flow = ControlFlow::Wait;

loop {
    // Pump the event loop - returns immediately if no events
    match winit_app.pump_eframe_app(&mut eventloop, None) {
        EframePumpStatus::Continue(next) => control_flow = next,
        EframePumpStatus::Exit(code) => break,
    }

    // If ControlFlow::Wait, event loop is dormant
    // Do other work here (e.g., tokio tasks, background processing)
    if control_flow == ControlFlow::Wait {
        // GUI is idle - no repaints scheduled
        // Can sleep or do background work
    }
}
```

---

## Key Differences from winit's `run_app_on_demand`

| Feature | winit `run_app_on_demand` | eframe `pump_eframe_app` |
|---------|--------------------------|--------------------------|
| API Style | Deprecated in 0.30, use `pump_app_events` | Uses `pump_app_events` internally |
| Return Value | `PumpStatus` enum | `EframePumpStatus` (wraps `ControlFlow`) |
| Integration | Direct winit integration | Works with eframe's `App` trait |
| Window Creation | Must create windows in `resumed()` | eframe handles window creation |

**Note**: winit 0.30 deprecated `run_on_demand` in favor of:
- `EventLoop::run_app_on_demand` (still available)
- `EventLoopExtPumpEvents::pump_app_events` (preferred)

---

## How to Make GUI Dormant (No Repaints)

### Strategy 1: Close/Hide Window + ControlFlow::Wait

1. When hiding window: `ctx.request_repaint()` stops being called
2. Event loop transitions to `ControlFlow::Wait` naturally (no pending redraws)
3. Event loop blocks until new events arrive (minimal CPU)

### Strategy 2: Explicit ControlFlow Management

```rust
// In your App::update:
fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
    if self.window_hidden {
        // Don't request repaint - event loop will become dormant
        return;
    }

    // Normal UI rendering...
    ctx.request_repaint_after_secs(0.1); // Keeps event loop active
}
```

---

## Examples from egui Repository

The PR #6750 added two examples:

### `external_eventloop` - Basic Integration
```rust
// examples/external_eventloop/src/main.rs
let eventloop = EventLoop::<UserEvent>::with_user_event().build()?;
eventloop.set_control_flow(ControlFlow::Poll);

let mut winit_app = eframe::create_native(..., &eventloop);
eventloop.run_app(&mut winit_app)?;
```

### `external_eventloop_async` - Tokio Integration (Linux only)
Shows how to run eframe alongside tokio's async executor in the same thread, enabling:
- `spawn_local` from UI without locks
- Shared data between UI and async tasks
- Single-thread execution

---

## Recommended Implementation Approach for LiteClip

Given LiteClip's requirements (background recording, tray icon, show window on demand):

### Option A: Replace `run_native` with `create_native` + `run_app`

```rust
// In main.rs, replace:
// eframe::run_native("LiteClip", native_options, app_creator)

// With:
let eventloop = EventLoop::<UserEvent>::with_user_event().build()?;
eventloop.set_control_flow(ControlFlow::Wait); // Dormant by default

let mut winit_app = eframe::create_native(
    "LiteClip",
    native_options,
    app_creator,
    &eventloop,
);

eventloop.run_app(&mut winit_app)?;
```

This allows:
- Tray icon events to wake the event loop via `EventLoopProxy::send_event()`
- Window visibility toggles without quitting
- Natural dormant state when window is hidden

### Option B: Manual Pumping (Advanced)

For more control, integrate with a custom loop:

```rust
let mut eventloop = EventLoop::<UserEvent>::with_user_event().build()?;
let mut winit_app = eframe::create_native(..., &eventloop);

loop {
    match winit_app.pump_eframe_app(&mut eventloop, Some(Duration::ZERO)) {
        EframePumpStatus::Continue(ControlFlow::Wait) => {
            // GUI dormant - check recording pipeline, handle tray events
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        EframePumpStatus::Continue(_) => {
            // GUI active - continue pumping
        }
        EframePumpStatus::Exit(_) => break,
    }
}
```

---

## Limitations

1. **iOS**: `pump_eframe_app` is not available (iOS requires owning the main thread)
2. **Windows async**: Cannot integrate with tokio on Windows (requires `WaitForMultipleObjectsEx`)
   - Solution: Run eframe on main thread, tokio on background thread
3. **Multiple windows**: `EframeWinitApplication` manages eframe's window; other windows need custom `ApplicationHandler`

---

## References

- **PR #6750**: https://github.com/emilk/egui/pull/6750 - Adds external event loop support
- **Issue #2875**: https://github.com/emilk/egui/issues/2875 - Original feature request
- **Issue #4709**: https://github.com/emilk/egui/issues/4709 - winit ApplicationHandler migration
- **winit 0.30 changelog**: https://rust-windowing.github.io/winit/winit/changelog/v0_30/
- **eframe docs**: https://docs.rs/eframe/0.33.3/eframe/

---

## Conclusion

**eframe 0.33.3 DOES support pause/resume semantics** through:
1. `create_native()` - creates app without blocking on `run_native`
2. `pump_eframe_app()` - manual event loop pumping (not available on iOS)
3. `ControlFlow::Wait` - natural dormant state when no repaints requested

For LiteClip's use case (background recorder with tray), **Option A** is recommended:
- Use `create_native` + `run_app` instead of `run_native`
- Set initial `ControlFlow::Wait`
- Wake via `EventLoopProxy::send_event()` when window should show
- Window hidden → no repaint requests → `ControlFlow::Wait` → dormant

This achieves the goal of aggressive CPU reduction without needing winit's deprecated `run_on_demand` API.
