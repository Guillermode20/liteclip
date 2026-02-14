# LiteClip Recorder — TODO

## Core Features
- [x] FFmpeg-based screen capture with `gdigrab`
- [x] Rolling segment buffer (configurable: 30s / 1m / 2m / 5m / 10m)
- [x] Auto-save clips to `~/Videos/LiteClip/` (no dialog)
- [x] Desktop audio capture via `dshow`
- [x] Lightweight egui GUI (Medal-style dark theme)
- [x] Global hotkey (configurable: F8, F9, F10, Ctrl+Shift+S, Alt+F9)
- [x] Quality/compression settings (Low / Medium / High / Ultra)
- [x] Framerate options (15 / 30 / 60 FPS)
- [x] Resolution scaling (Native / 1080p / 720p / 480p)
- [x] Audio device auto-detection and selection
- [x] Output folder picker
- [x] Auto-detect FFmpeg on PATH with error state
- [x] Buffer fill progress bar
- [x] Pulsing REC indicator
- [x] Settings panel with gear icon toggle

## Nice-to-Have (Future)
- [ ] System tray icon (minimize to tray)
- [ ] Microphone capture + mixing
- [ ] Persistent settings (save to config file)
- [ ] Monitor/region selection (multi-monitor)
- [ ] Notification toast on save
- [ ] Auto-start with Windows
- [x] Custom encoder selection (NVENC, AMF, QSV)
- [ ] Clip editor / trimmer
