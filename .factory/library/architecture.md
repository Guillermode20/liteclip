# Architecture: GUI Thread CPU Reduction

## System Overview

LiteClip Replay is a native Windows screen recorder with the following thread architecture:

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                           THREAD ARCHITECTURE                                │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                              │
│   Main Thread (Tokio async runtime)                                          │
│   ├── Event Loop (tokio::select!)                                            │
│   │   ├── Platform events from crossbeam channel                             │
│   │   ├── Health monitoring (enforce_pipeline_health)                        │
│   │   └── Config I/O                                                         │
│   │                                                                          │
│   Platform Thread (std::thread)                                              │
│   ├── Windows message loop                                                   │
│   ├── Hotkey handling (global hooks)                                         │
│   └── Tray icon management                                                   │
│   │                                                                          │
│   GUI Thread (std::thread + eframe/winit)                                    │
│   ├── Overlay window (toasts)                                                │
│   ├── Settings (deferred viewport)                                           │
│   └── Gallery (deferred viewport)                                            │
│   │                                                                          │
│   Recording Pipeline (spawned by AppState)                                   │
│   ├── Capture Thread (DXGI Desktop Duplication)                              │
│   ├── Encode Thread (NVENC/AMF/QSV/SW)                                       │
│   └── Buffer (lock-free SPMC ring)                                           │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
```

## GUI Thread Current Behavior

### Event Loop Pattern for EventLoopProxy Capture

**Important:** To capture `EventLoopProxy` for wake-on-message, you must use `eframe::create_native` + `run_app` pattern instead of `eframe::run_native`. The standard `run_native` with `event_loop_builder` callback does NOT expose the proxy because the event loop is built internally.

```rust
// Pattern for capturing EventLoopProxy (eframe 0.33.3)
let native = eframe::create_native(
    "Overlay",
    NativeOptions {
        event_loop_builder: Some(event_loop_builder),
        with_any_thread: true, // Required for Windows off-main-thread
        ..
    },
    Box::new(|cc| Ok(Box::new(GuiManagerApp::new(cc, initial_state)))),
)?;
let proxy = native.event_loop.create_proxy();
// Store proxy in GuiManagerState before running
native.run_app(app)?;
```

This pattern was discovered through docs.rs exploration and is essential for any future GUI wake mechanism modifications.

### Problem: Periodic Polling

The GUI thread runs with a 1x1 pixel overlay window. Even when truly idle:
- `request_repaint_after(100ms)` is called every update cycle
- Event loop polls at 10 Hz even with no visible windows
- Estimated CPU usage: 0.5-2% from unnecessary polling

### Code Location

```rust
// src/gui/manager.rs:430-433
if show_settings || show_gallery {
    ctx.request_repaint();
} else {
    ctx.request_repaint_after(Duration::from_millis(IDLE_REPAINT_MS)); // 100ms
}
```

## Solution: Dormant Event Loop

### Approach

1. **Stop periodic repaints when truly idle**: Remove the 100ms timer when no windows/toasts visible
2. **Use EventLoopProxy for wake-on-message**: When `GuiMessage` arrives, wake the event loop instantly
3. **Natural dormancy**: winit's `ControlFlow::Wait` makes event loop dormant when no repaints pending

### Thread Communication

```
┌──────────────┐     GuiMessage      ┌──────────────┐
│ Platform     │ ──────────────────▶ │ GUI Thread   │
│ Thread       │                     │ ( dormant)   │
│              │                     │              │
│              │     EventLoopProxy  │              │
│              │ ──────────────────▶ │ Wakes loop   │
└──────────────┘                     └──────────────┘
```

### Key Components

| Component | Location | Change Needed |
|-----------|----------|---------------|
| `GuiManagerState` | `manager.rs` | Add `EventLoopProxy` field |
| `spawn_gui_manager_thread` | `manager.rs` | Capture proxy from eframe |
| `send_gui_message` | `manager.rs` | Send wake event via proxy |
| `GuiManagerApp::update` | `manager.rs` | Conditional repaint logic |
| `NativeOptions` | `manager.rs` | Keep `with_any_thread(true)` |

## Invariants

1. **Thread Independence**: GUI thread changes must NOT affect recording pipeline threads
2. **Wake Latency**: Event loop must wake <50ms after `GuiMessage` arrives
3. **No Deadlocks**: EventLoopProxy send must not block
4. **Memory Stability**: No memory growth during idle periods

## Testing Strategy

- **Unit tests**: Idle state detection, conditional repaint logic
- **Manual tests**: CPU measurement, timing validation (native app limitation)
- **Regression tests**: Recording pipeline CPU unchanged, hotkey response unchanged
