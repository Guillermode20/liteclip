# Winit Threading Model Research

## Summary of the Constraint

Winit imposes strict threading and lifecycle constraints on the `EventLoop`:

### Core Constraints

1. **Single EventLoop per process**: Creating multiple `EventLoop` instances is explicitly not supported and will panic with the message: "Creating EventLoop multiple times is not supported."

2. **Main thread requirement**: The `EventLoop` must be created on the main thread by default. This is enforced via a `OnceCell` check in `EventLoopBuilder::build()`. Attempting to create the event loop off the main thread will panic on most platforms.

3. **Non-restartable**: Once `EventLoop::run()` exits, the event loop cannot be restarted. The standard `run()` method takes ownership of the event loop and returns only when the application exits.

4. **Not Send/Sync**: `EventLoop` is neither `Send` nor `Sync`, preventing cross-thread access. This is documented: "Note that this cannot be shared across threads (due to platform-dependant logic forbidding it)."

### Why These Constraints Exist

The constraints stem from platform-specific requirements:

- **macOS/AppKit**: Requires the event loop to run on the main thread. `NSApplication` is designed to run once per process. Dropping the event loop drops the connection to the display server, destroying all windows.

- **Wayland/X11**: Dropping an event loop drops the connection to the display server. On Wayland, all windows would be destroyed immediately.

- **Windows**: Windows message pump is thread-specific, and re-entrant event handlers can cause panics.

The winit team intentionally imposes these restrictions across all platforms "to eliminate any nasty surprises when porting to platforms that require it."

## Documented Workarounds

### 1. `pump_events` / `pump_app_events` (Recommended for External Loop Integration)

Available via `EventLoopExtPumpEvents` trait in `winit::platform::pump_events`. This allows pumping events within an external event loop:

```rust
use winit::platform::pump_events::EventLoopExtPumpEvents;

let mut event_loop = EventLoop::new();
loop {
    // Pump winit events alongside your other work
    event_loop.pump_app_events(timeout, |event, elwt| {
        // Handle event
    });
    // Do other work here
}
```

**Limitations**: The event loop still must be created once and exist for the lifetime of the application.

### 2. `run_app_on_demand` (Experimental)

Available in `winit::platform::run_on_demand::EventLoopExtRunOnDemand`. This allows running the event loop, exiting, and potentially re-running:

```rust
use winit::platform::run_on_demand::EventLoopExtRunOnDemand;

let event_loop = EventLoop::new();
event_loop.run_app_on_demand(&mut my_app);
// Can potentially run again later
event_loop.run_app_on_demand(&mut my_app);
```

**Status**: This is a newer API (added in winit 0.30) and may have platform-specific issues. It was added to address the "Can't create event loop ondemand" issue (GitHub #2431).

### 3. Hide/Show Windows Instead of Recreating EventLoop

The recommended pattern for "GUI on demand" is to:
1. Create the `EventLoop` once at startup
2. Create windows as needed
3. Hide windows when not needed (`window.set_visible(false)`)
4. Show windows when needed again (`window.set_visible(true)`)
5. Never drop the `EventLoop` until application exit

This works well for tray applications or background apps that occasionally show UI.

### 4. Window on Separate Thread with `with_any_thread` (Windows-only)

On Windows, `EventLoopBuilder` has a `with_any_thread()` method that allows creating the event loop off the main thread:

```rust
use winit::platform::windows::EventLoopBuilderExtWindows;

let event_loop = EventLoopBuilder::new()
    .with_any_thread(true)
    .build();
```

**Limitations**: 
- Only available on Windows
- macOS and Linux do not support this
- Still limited to one EventLoop per process

### 5. `EventLoopProxy` for Cross-Thread Communication

Use `EventLoopProxy` to wake up the event loop from another thread:

```rust
let proxy = event_loop.create_proxy();
// Send events from another thread
proxy.send_event(MyCustomEvent::Something);
```

## Alternative Approaches

### 1. `winit-modular` Crate

A third-party crate that provides proxy event loops which can run simultaneously on separate threads:

- **GitHub**: https://github.com/Jakobeha/winit-modular
- **Approach**: Creates proxy event loops that forward calls to a hidden main event loop
- **Tradeoffs**: Performance penalty due to cross-thread communication; requires calling `winit_modular::run` on main thread first

```rust
winit_modular::run(|| block_on(async {
    let event_loop = EventLoop::new().await;
    // Can create multiple proxy event loops
    event_loop.run_async(|event, control_flow, _| {
        // Handle events
    }).await;
}));
```

### 2. Window Visibility Pattern (Recommended)

For background applications like screen recorders:

```rust
fn main() {
    let event_loop = EventLoop::new();
    let window = WindowBuilder::new().build(&event_loop).unwrap();
    
    // Initially hidden for background operation
    window.set_visible(false);
    
    event_loop.run(move |event, elwt, control_flow| {
        match event {
            Event::UserEvent(ShowGui) => {
                window.set_visible(true);
            }
            Event::UserEvent(HideGui) => {
                window.set_visible(false);
            }
            // Handle other events...
        }
    });
}
```

### 3. Separate Process for GUI

Spawn a subprocess for the GUI component when needed. This avoids the winit constraints entirely but requires IPC between processes.

### 4. Use Different Windowing Library

For applications needing more flexible threading models:
- **raw-window-handle**: Provides raw window handles for integration with other systems
- **glutin**: OpenGL context creation (built on winit, inherits same constraints)
- **SDL2**: More flexible threading model, but requires C bindings

## Relevant Issues and Discussions

| Issue | Summary | Link |
|-------|---------|------|
| #1585 | Panic on multiple event loops with multiple windows | https://github.com/rust-windowing/winit/issues/1585 |
| #2431 | Can't create event loop ondemand | https://github.com/rust-windowing/winit/issues/2431 |
| #2706 | Integration with external event loops | https://github.com/rust-windowing/winit/issues/2706 |
| #2767 | PR adding `run_ondemand` API | https://github.com/rust-windowing/winit/pull/2767 |
| #2885 | Creating EventLoop multiple times not supported | https://github.com/rust-windowing/winit/issues/2885 |
| #2900 | EventLoop 3.0 changes tracking | https://github.com/rust-windowing/winit/issues/2900 |

## Recommendations for LiteClip

Given LiteClip's architecture (background screen recorder with on-demand gallery UI):

1. **Best approach**: Keep the single `EventLoop` alive for the entire application lifetime. Use window visibility toggling (`set_visible`) to show/hide the gallery window rather than destroying and recreating it.

2. **For tray-only operation**: Create the window initially hidden. The event loop can remain active while the window is hidden, handling hotkey events via `EventLoopProxy`.

3. **Avoid**: Attempting to create/destroy/recreate the event loop. This is explicitly unsupported and will panic.

4. **If truly needed**: Consider using `run_app_on_demand` (winit 0.30+) if you need to temporarily close all windows and later re-open them, but test thoroughly on all target platforms as some (particularly macOS) may have issues.

## Key Documentation References

- EventLoop docs: https://docs.rs/winit/latest/winit/event_loop/struct.EventLoop.html
- EventLoopBuilder docs: https://docs.rs/winit/latest/winit/event_loop/struct.EventLoopBuilder.html
- pump_events platform docs: https://docs.rs/winit/latest/winit/platform/pump_events/
- run_on_demand platform docs: https://docs.rs/winit/latest/winit/platform/run_on_demand/
- winit changelog v0.30: https://rust-windowing.github.io/winit/winit/changelog/v0_30/
