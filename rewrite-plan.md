# liteclip-replay — Architecture & Development Plan

A lightweight, open-source game clip recorder built in Rust for Windows. Background replay buffer, hotkey-triggered clip saving, GPU-accelerated encoding, and a native GUI — all in a single compiled binary with minimal resource overhead.

---

## 1. Core Design Principles

- **Minimal idle footprint**: <50MB RAM, <1% CPU when buffering in background
- **Zero runtime dependencies**: single static binary, no Electron, no browser engine
- **GPU-first encoding**: offload to hardware encoders (NVENC / AMF / QSV)
- **Replay buffer model**: continuously record the last N seconds, save on demand
- **Windows-only**: no cross-platform abstraction tax, direct Win32/COM API usage

---

## 2. High-Level Architecture

```
┌──────────────────────────────────────────────────────┐
│                     liteclip-replay                        │
├──────────┬──────────┬───────────┬────────────────────┤
│  Capture │  Encode  │  Buffer   │   GUI / Overlay    │
│  Engine  │  Pipeline│  Manager  │                    │
├──────────┼──────────┼───────────┼────────────────────┤
│ WGC /    │ FFmpeg   │ Ring      │ iced               │
│ DXGI Dup │ (libav)  │ Buffer    │ (native, wgpu)     │
│ WASAPI   │ NVENC    │ in-memory │                    │
│          │ AMF/QSV  │           │                    │
└──────────┴──────────┴───────────┴────────────────────┘
         ↕               ↕              ↕
    ┌──────────┐   ┌───────────┐  ┌──────────────┐
    │ Win32    │   │ Clip      │  │ Settings     │
    │ Msg Loop │   │ Finaliser │  │ (TOML)       │
    │ (hidden) │   │           │  │              │
    └──────────┘   └───────────┘  └──────────────┘
```

---

## 3. Technology Stack

| Layer | Choice | Rationale |
|---|---|---|
| Language | **Rust (stable)** | Memory safety, zero-cost abstractions, single binary |
| GUI | **iced** | Native wgpu rendering, Elm architecture, no web engine |
| Screen capture | **DXGI Desktop Duplication** (Phase 1), **Windows.Graphics.Capture** (Phase 3) | DXGI is simpler to bootstrap; WGC is lower overhead + better anti-cheat compat |
| Audio capture | **WASAPI loopback** via `windows` crate | System audio + mic, lowest latency Windows audio API |
| Video encoding | **FFmpeg C API** via `ffmpeg-next` + `ffmpeg-sys-next` for hwaccel plumbing | NVENC, AMF, QSV + software x264 fallback |
| Container | **Standard MP4** via FFmpeg muxer | Universal compatibility. Writing from complete buffer so fragmented MP4 unnecessary |
| Hotkeys | **Win32 `RegisterHotKey`** on dedicated hidden HWND thread | Works during exclusive fullscreen |
| Overlay | **DirectComposition** layered window | Zero-impact transparent overlay |
| Config | **TOML** via `serde` + `toml` | Human-readable, Rust-idiomatic |
| Logging | **tracing** + **tracing-subscriber** | Structured, async-safe |
| Installer | **NSIS** or **WiX** via `cargo-wix` | Standard Windows installer |
| Tray icon | **tray-icon** crate (shares hidden HWND message pump) | System tray integration |

---

## 4. Module Breakdown

### 4.1 Capture Engine (`capture/`)

**Responsibility**: Acquire raw frames from the desktop/game and audio from system output + microphone.

```
capture/
├── mod.rs              // CaptureBackend trait
├── wgc.rs              // Windows.Graphics.Capture (Phase 3, Win10 1903+)
├── dxgi.rs             // DXGI Desktop Duplication (Phase 1, broad compat)
├── audio.rs            // WASAPI loopback for system audio
├── mic.rs              // WASAPI mic capture
└── mixer.rs            // Mix system + mic streams with volume controls
```

**Key trait**:
```rust
pub trait CaptureBackend: Send + 'static {
    fn start(&mut self, config: CaptureConfig) -> Result<()>;
    fn stop(&mut self);
    fn frame_rx(&self) -> Receiver<CapturedFrame>;
}

pub struct CapturedFrame {
    pub texture: ID3D11Texture2D,   // GPU-resident, no CPU copy
    pub timestamp: i64,             // QPC timestamp
    pub resolution: (u32, u32),
}
```

**DXGI Desktop Duplication (Phase 1)**:
- `IDXGIOutputDuplication::AcquireNextFrame` in an event-driven loop
- Returns `ID3D11Texture2D` — stays on GPU
- Broader game compatibility, simpler to get working
- Forward texture handle to encoder via crossbeam channel

**Windows.Graphics.Capture (Phase 3 upgrade)**:
1. Create `Direct3D11CaptureFramePool` with `CreateFreeThreaded`
2. Use `GraphicsCaptureSession` to capture target window or monitor
3. Callback-driven — lower overhead than DXGI polling
4. Better anti-cheat compatibility, HDR support

**Audio pipeline**:
- WASAPI loopback: `IAudioClient` in shared mode, `AUDCLNT_STREAMFLAGS_LOOPBACK`
- Microphone: separate `IAudioClient` capture stream
- Mixer combines both with configurable per-source volume
- Audio encoded as AAC via FFmpeg, packets interleaved with video in ring buffer
- Sync via shared QPC (`QueryPerformanceCounter`) timestamps on both streams

### 4.2 Encode Pipeline (`encode/`)

**Responsibility**: Encode raw D3D11 textures into H.264/H.265/AV1 using hardware acceleration.

```
encode/
├── mod.rs              // Encoder trait + hw detection factory
├── hw_encoder.rs       // NVENC / AMF / QSV via FFmpeg hwaccel
├── sw_encoder.rs       // x264 software fallback
├── gpu_transfer.rs     // ID3D11Texture2D → AVFrame bridge (unsafe FFmpeg-sys plumbing)
├── cpu_readback.rs     // Staging texture CPU fallback path
└── presets.rs          // Performance / Balanced / Quality profiles
```

**GPU-accelerated path**:
```
ID3D11Texture2D (from capture)
    → AVHWFramesContext (D3D11VA)
        → FFmpeg hardware encoder (NVENC/AMF/QSV)
            → EncodedPacket (H.264 NAL units)
```

**Important implementation note**: `ffmpeg-next`'s Rust bindings don't have ergonomic support for `AVHWFramesContext` creation and D3D11 texture mapping. The `gpu_transfer.rs` module will need to use `ffmpeg-sys-next` (raw C bindings) with unsafe blocks for:
- `av_hwdevice_ctx_create` (D3D11VA device)
- `av_hwframe_ctx_alloc` + `av_hwframe_ctx_init` (frame pool)
- Mapping `ID3D11Texture2D` into `AVFrame.data[0]` / `AVFrame.data[1]`

Consider a thin C shim compiled via `cc` in `build.rs` if the unsafe surface area gets too large.

**CPU readback fallback** (ready from Phase 1):
```
ID3D11Texture2D (from capture)
    → ID3D11DeviceContext::CopyResource to staging texture
        → Map → memcpy to AVFrame (CPU memory)
            → Software or hardware encoder
```

This is slower (~2-5ms per frame copy) but guaranteed to work and de-risks the MVP. Phase 1 can ship with CPU readback while the zero-copy GPU path is hardened.

**Hardware detection priority**:
1. NVIDIA → `h264_nvenc` / `hevc_nvenc` (probe encoder availability)
2. AMD → `h264_amf` / `hevc_amf`
3. Intel → `h264_qsv` / `hevc_qsv`
4. None → `libx264` software (with toast warning about CPU impact)

**Encoding defaults**:
- H.264 High Profile, CBR 20 Mbps @ 1080p30
- Keyframe interval: 1 second (precise clip boundaries)
- B-frames: 0 (lower latency, simpler buffer seeking)
- Lookahead: 0 (minimal encode latency)

### 4.3 Ring Buffer (`buffer/`)

**Responsibility**: Maintain a rolling window of the last N seconds of encoded data in memory.

```
buffer/
├── mod.rs
├── ring.rs             // Ring buffer for encoded packets
├── index.rs            // Keyframe index for fast seeking
└── budget.rs           // Memory budget enforcement
```

```rust
use bytes::Bytes;

pub struct ReplayBuffer {
    packets: VecDeque<EncodedPacket>,
    duration: Duration,             // e.g. 120 seconds
    max_memory_bytes: usize,        // e.g. 512MB
    keyframe_index: BTreeMap<i64, usize>,  // QPC → packet index
    total_bytes: usize,
}

pub struct EncodedPacket {
    pub data: Bytes,               // Reference-counted, cheap to clone
    pub pts: i64,                  // Presentation timestamp (QPC-based)
    pub dts: i64,                  // Decode timestamp
    pub is_keyframe: bool,
    pub stream: StreamType,        // Video | SystemAudio | Microphone
}
```

**Snapshot for saving**: using `Bytes` means cloning the `VecDeque` for a save snapshot is just reference count bumps — genuinely sub-millisecond regardless of buffer size. The live buffer and save task share the underlying byte data until packets are evicted from the ring.

**Memory budget**: H.264 @ 20 Mbps, 120 seconds ≈ 300MB. Packet eviction from the front when `total_bytes` exceeds the configured cap. Duration degrades gracefully rather than OOMing.

### 4.4 Clip Finaliser (`clip/`)

**Responsibility**: On save trigger, snapshot buffer → mux to standard MP4 → write to disk.

```
clip/
├── mod.rs
├── muxer.rs            // Standard MP4 muxing via FFmpeg
├── trimmer.rs          // Optional trim before save
├── metadata.rs         // Embed game name, timestamp, resolution
└── thumbnail.rs        // Extract keyframe → JPEG thumbnail
```

**Save flow**:
1. Hotkey pressed → acquire read lock → snapshot packet buffer (sub-ms with `Bytes`)
2. Spawn `tokio::task::spawn_blocking` for mux work
3. Seek to nearest keyframe at `now - clip_duration`
4. Mux video + audio streams into standard MP4 via `avformat_write_header` / `av_interleaved_write_frame`
5. Extract first keyframe → decode to RGB → encode JPEG thumbnail
6. Write to `save_directory/{game_name}/{timestamp}.mp4`
7. Fire Windows toast notification with thumbnail preview

**Why standard MP4, not fragmented**: writing from a complete in-memory buffer, not streaming to disk during recording. Standard MP4 with `moov` at the end (or `faststart` moved to front) is simpler and has broader player compatibility. Fragmented MP4 would only matter for a future "continuous write to disk" mode.

**Game name detection**: read the foreground window title via `GetForegroundWindow` + `GetWindowTextW`. Heuristic cleanup to strip resolution/FPS suffixes. Falls back to process name via `GetWindowThreadProcessId` → `OpenProcess` → `QueryFullProcessImageNameW`. User can rename in gallery.

### 4.5 GUI (`gui/`)

```
gui/
├── mod.rs
├── app.rs              // iced Application impl
├── views/
│   ├── dashboard.rs    // Status indicator, recent clips, buffer stats
│   ├── settings.rs     // Full settings (video, audio, hotkeys, storage)
│   ├── gallery.rs      // Clip browser with thumbnail grid
│   └── trimmer.rs      // Simple trim editor (set in/out points)
├── overlay.rs          // Recording indicator (DirectComposition window)
├── tray.rs             // System tray glue (events from hidden HWND)
└── theme.rs            // Dark theme matching Windows 11 aesthetic
```

**System tray behaviour**:
- App starts minimised to tray by default
- Tray icon context menu: Open Dashboard / Pause Recording / Settings / Quit
- Left-click tray icon → open/focus dashboard
- Close button minimises to tray (not quit)

**Overlay** (separate transparent Win32 window):
- Small coloured dot in corner (green = buffering, red = saving, hidden = paused)
- "Clip Saved!" flash animation on save
- Created as `WS_EX_TRANSPARENT | WS_EX_LAYERED | WS_EX_TOPMOST` — click-through, always on top
- Rendered via DirectComposition or simple GDI+ for minimal overhead

### 4.6 Hidden HWND & Message Pump (`platform/`)

**This is critical infrastructure** that both hotkeys and the tray icon depend on.

```
platform/
├── mod.rs
├── msg_loop.rs         // Hidden HWND + GetMessage/DispatchMessage pump
├── hotkeys.rs          // RegisterHotKey on hidden HWND
└── tray.rs             // tray-icon message integration
```

**Problem**: iced uses winit internally, which owns its own event loop. You cannot inject `WM_HOTKEY` or tray icon messages into it.

**Solution**: spawn a dedicated thread with a hidden `HWND` and a minimal Win32 message pump:

```rust
pub fn spawn_platform_thread(
    hotkey_config: HotkeyConfig,
    event_tx: Sender<AppEvent>,  // crossbeam channel to iced app
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let hwnd = create_hidden_window();

        // Register global hotkeys on this HWND
        register_hotkeys(hwnd, &hotkey_config);

        // tray-icon also processes messages via this pump
        setup_tray_icon(hwnd);

        // Message loop
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, HWND::default(), 0, 0).as_bool() {
            match msg.message {
                WM_HOTKEY => {
                    let action = hotkey_id_to_action(msg.wParam);
                    event_tx.send(AppEvent::Hotkey(action)).ok();
                }
                // tray-icon callback messages handled here too
                _ => {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        }
    })
}
```

This single thread services both `RegisterHotKey` and `tray-icon` message needs. Events are forwarded to the iced app via a crossbeam channel, which iced can poll via a `Subscription`.

**Default hotkey bindings** (Medal-compatible):
| Action | Default | Configurable |
|---|---|---|
| Save clip | `Alt+F9` | Yes |
| Toggle buffer | `Alt+F10` | Yes |
| Screenshot | `Alt+F11` | Yes |
| Open gallery | `Alt+G` | Yes |

### 4.7 Configuration (`config/`)

```toml
# liteclip-replay.toml (stored in %APPDATA%/liteclip-replay/)

[general]
replay_duration_secs = 120
save_directory = "~/Videos/liteclip-replay"
auto_start_with_windows = true
start_minimised = true
notifications = true
auto_detect_game = true

[video]
resolution = "native"           # native, 1080p, 720p
framerate = 30                  # 30, 60
codec = "h264"                  # h264, h265, av1
bitrate_mbps = 20
encoder = "auto"                # auto, nvenc, amf, qsv, software

[audio]
capture_system = true
capture_mic = false
mic_device = "default"
mic_volume = 80                 # 0-100
system_volume = 100

[hotkeys]
save_clip = "Alt+F9"
toggle_recording = "Alt+F10"
screenshot = "Alt+F11"
open_gallery = "Alt+G"

[advanced]
memory_limit_mb = 512
gpu_index = 0
keyframe_interval_secs = 1
overlay_enabled = true
overlay_position = "top-left"   # top-left, top-right, bottom-left, bottom-right
use_cpu_readback = false        # Force CPU readback path (debug/compat)
```

---

## 5. Threading Model

```
Thread 1 (main): GUI
    └── iced event loop (winit-owned)
    └── Receives AppEvents via crossbeam channel as iced Subscription

Thread 2: Win32 platform thread (hidden HWND)
    └── GetMessage/DispatchMessage pump
    └── Handles: RegisterHotKey WM_HOTKEY, tray-icon messages
    └── Forwards events to iced via crossbeam Sender

Thread 3: Frame capture
    └── DXGI/WGC frame acquisition at target FPS
    └── Sends ID3D11Texture2D handles via crossbeam channel

Thread 4: Video encoder
    └── Receives textures → FFmpeg hwaccel encode (or CPU readback path)
    └── Pushes encoded packets into ring buffer

Thread 5: Audio capture + encode
    └── WASAPI loopback + mic capture
    └── AAC encode → push audio packets to ring buffer

Thread N (spawned on demand): Clip saver
    └── Snapshots buffer → muxes standard MP4 → disk write
    └── One per save request, short-lived
```

**Sync primitive choices**:
- Capture → Encoder: `crossbeam::channel::bounded(4)` (backpressure if encoder falls behind)
- Ring buffer: `parking_lot::RwLock` (readers rarely contend with the single writer)
- Config changes: `arc_swap::ArcSwap` (lock-free reads from hot threads)
- Platform → GUI: `crossbeam::channel::unbounded` (hotkey/tray events are infrequent)

---

## 6. Performance Budget

| Metric | Target | Strategy |
|---|---|---|
| Idle CPU | <1% | Capture loop event-driven, not busy-polling |
| Active CPU (GPU encode) | <5% | Hardware encoding, no CPU pixel ops |
| Active CPU (SW fallback) | <25% | x264 veryfast preset, with user warning |
| RAM (buffer) | 50–512MB | Configurable, proportional to duration × bitrate |
| RAM (app overhead) | <30MB | No web engine, no GC, no runtime |
| Disk I/O | Burst on save only | No continuous writes while buffering |
| GPU overhead | <3% | NVENC/AMF use dedicated encoder silicon |
| Binary size | <15MB | Static link, fat LTO, strip symbols, `opt-level = 3` |
| Startup time | <500ms | No JIT, no runtime init, lazy GUI render |
| Clip save time | <2s for 2min | Sequential write, standard MP4 |

---

## 7. Development Phases

### Phase 1 — Core Recording MVP (Weeks 1–4)
- [ ] D3D11 device creation + DXGI Desktop Duplication capture
- [ ] CPU readback path (`CopyResource` → staging texture → `Map` → `AVFrame`)
- [ ] NVENC H.264 encoding via `ffmpeg-next` / `ffmpeg-sys-next`
- [ ] In-memory ring buffer with `Bytes`-backed packets and eviction
- [ ] Hidden HWND thread with `RegisterHotKey` to trigger clip save
- [ ] Standard MP4 muxing and disk write
- [ ] TOML config loading from `%APPDATA%`
- [ ] CLI-only interface for testing (no GUI yet)
- [ ] Basic tracing/logging

### Phase 2 — Audio & GUI (Weeks 5–8)
- [ ] WASAPI loopback system audio capture
- [ ] AAC audio encoding + audio/video interleaving in buffer
- [ ] PTS-based A/V sync with QPC shared clock
- [ ] Microphone capture + mixer with volume controls
- [ ] iced GUI: dashboard with status + recent clips
- [ ] iced GUI: settings panel (all config options)
- [ ] System tray icon via `tray-icon` (sharing hidden HWND pump)
- [ ] Windows toast notification on clip save

### Phase 3 — Polish & Compatibility (Weeks 9–12)
- [ ] Zero-copy GPU encode path (D3D11 texture → AVHWFramesContext → NVENC)
- [ ] Windows.Graphics.Capture as primary capture backend
- [ ] AMD AMF + Intel QSV encoder backends
- [ ] Software x264 fallback with performance warning toast
- [ ] Recording indicator overlay (DirectComposition)
- [ ] Clip gallery with thumbnail grid
- [ ] In-app trim (set in/out points before final save)
- [ ] Auto-detect game name from window title / process name
- [ ] Auto-start on Windows boot (registry / startup folder)
- [ ] NSIS or WiX installer + GitHub releases

### Phase 4 — Advanced Features (Weeks 13+)
- [ ] H.265 / AV1 encoding options
- [ ] Resolution downscaling (capture native → encode at 720p/1080p)
- [ ] Multi-monitor / specific window capture selection
- [ ] Clip bookmarking / tagging system
- [ ] Configurable clip length (save last 15s, 30s, 60s, 120s)
- [ ] Instant replay window (preview last N seconds without saving)
- [ ] Discord/social sharing integration
- [ ] Auto-update system (via GitHub releases API)
- [ ] HDR capture + tone-mapping for SDR output

---

## 8. Cargo Dependencies

```toml
[dependencies]
# GUI
iced = { version = "0.13", features = ["tokio", "image"] }
tray-icon = "0.19"
muda = "0.15"                   # Context menus for tray

# Windows APIs
windows = { version = "0.58", features = [
    "Win32_Graphics_Dxgi",
    "Win32_Graphics_Dxgi_Common",
    "Win32_Graphics_Direct3D11",
    "Win32_Graphics_Direct3D",
    "Graphics_Capture",
    "Graphics_DirectX_Direct3D11",
    "Win32_Media_Audio",
    "Win32_UI_Input_KeyboardAndMouse",
    "Win32_UI_WindowsAndMessaging",
    "Win32_Foundation",
    "Win32_System_Com",
    "Win32_System_Threading",
    "Win32_System_Performance",
]}

# FFmpeg
ffmpeg-next = "7"               # High-level encode/mux API
ffmpeg-sys-next = "7"           # Raw C bindings for AVHWFramesContext plumbing

# Ref-counted byte buffers
bytes = "1"

# Async
tokio = { version = "1", features = ["rt-multi-thread", "macros", "sync"] }

# Concurrency
crossbeam = "0.8"
parking_lot = "0.12"
arc-swap = "1"

# Config
serde = { version = "1", features = ["derive"] }
toml = "0.8"

# Logging
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }

# Image (thumbnails)
image = "0.25"

# Notifications
winrt-notification = "0.6"      # Windows toast notifications

# Directories
dirs = "5"

# Error handling
anyhow = "1"
thiserror = "2"

# NOTE: Pin exact versions at project init via `cargo add`.
# Versions listed here are approximate — fast-moving crates like
# tray-icon, muda, and winrt-notification may have newer releases.

[build-dependencies]
cc = "1"                        # Optional: compile C shim for FFmpeg hwaccel bridge

[profile.release]
opt-level = 3
lto = "fat"
codegen-units = 1
strip = true
panic = "abort"
```

---

## 9. Risks & Mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| FFmpeg static linking on Windows | Complex build setup | Vendor a pre-built static FFmpeg with only needed codecs. Use `vcpkg` or ship `.lib` files in repo. `build.rs` handles detection |
| `ffmpeg-next` lacks hwaccel ergonomics | Lots of unsafe code | Use `ffmpeg-sys-next` for hw plumbing, or a C shim via `cc`. Ship CPU readback from day one as fallback |
| Anti-cheat blocking DXGI/WGC | Some games won't capture | WGC (Phase 3) has better anti-cheat compat. Document known issues |
| D3D11 texture sharing capture ↔ encoder | Edge case corruption | `Flush` + fence sync. CPU readback fallback behind `use_cpu_readback` config flag |
| WASAPI audio format mismatches | Crackling / wrong sample rate | Negotiate shared-mode format, resample to 48kHz AAC input via FFmpeg `swr` |
| Audio/video drift over long sessions | Out-of-sync clips | QPC timestamps on both streams. Periodic drift correction in muxer (insert silence / drop frames) |
| iced can't host hotkeys or tray messages | Broken hotkeys/tray | Dedicated hidden HWND thread handles both. Events forwarded via crossbeam channel |
| Game window title unreliable | Wrong game labels | Fallback to process executable name. Allow manual override in gallery |

---

## 10. Project Structure

```
liteclip-replay/
├── Cargo.toml
├── build.rs                    // FFmpeg lib detection, optional C shim compilation
├── liteclip-replay.toml.example
├── LICENSE                     // MIT OR Apache-2.0
├── README.md
├── src/
│   ├── main.rs                 // Entry point, init logging, launch app
│   ├── app.rs                  // Application lifecycle, thread orchestration
│   ├── capture/
│   │   ├── mod.rs              // CaptureBackend trait
│   │   ├── wgc.rs              // Windows.Graphics.Capture
│   │   ├── dxgi.rs             // DXGI Desktop Duplication
│   │   ├── audio.rs            // WASAPI loopback
│   │   ├── mic.rs              // WASAPI mic capture
│   │   └── mixer.rs            // Audio stream mixer
│   ├── encode/
│   │   ├── mod.rs              // Encoder trait + hw detection factory
│   │   ├── hw.rs               // Hardware encoder (NVENC/AMF/QSV)
│   │   ├── sw.rs               // Software fallback (x264)
│   │   ├── gpu_transfer.rs     // D3D11 → AVFrame bridge (ffmpeg-sys-next unsafe)
│   │   ├── cpu_readback.rs     // Staging texture → Map → AVFrame fallback
│   │   └── presets.rs
│   ├── buffer/
│   │   ├── mod.rs
│   │   ├── ring.rs             // VecDeque<EncodedPacket> with Bytes
│   │   ├── index.rs            // Keyframe BTreeMap index
│   │   └── budget.rs           // Memory cap enforcement
│   ├── clip/
│   │   ├── mod.rs
│   │   ├── muxer.rs            // Standard MP4 muxing
│   │   ├── trimmer.rs
│   │   ├── metadata.rs
│   │   └── thumbnail.rs
│   ├── gui/
│   │   ├── mod.rs
│   │   ├── app.rs              // iced Application impl
│   │   ├── views/
│   │   │   ├── dashboard.rs
│   │   │   ├── settings.rs
│   │   │   ├── gallery.rs
│   │   │   └── trimmer.rs
│   │   ├── overlay.rs          // DirectComposition overlay window
│   │   └── theme.rs
│   ├── platform/
│   │   ├── mod.rs
│   │   ├── msg_loop.rs         // Hidden HWND + message pump thread
│   │   ├── hotkeys.rs          // RegisterHotKey wrapper
│   │   └── tray.rs             // tray-icon integration
│   ├── config.rs               // TOML serde structs
│   └── d3d.rs                  // D3D11 device + context init
├── shim/                       // Optional C shim for FFmpeg hwaccel bridge
│   └── hwframe_bridge.c
├── assets/
│   ├── icon.ico
│   └── icon.png
├── ffmpeg/                     // Vendored FFmpeg static libs (gitignored or fetched by build.rs)
├── installer/
│   └── liteclip-replay.nsi
└── tests/
    ├── buffer_tests.rs
    └── encode_tests.rs
```