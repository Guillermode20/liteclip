# Manual Testing Guide: GUI Thread CPU Reduction

This guide provides instructions for manual validation of the GUI thread CPU reduction implementation. All VAL-GUI assertions require human manual testing due to the native Windows GUI surface.

## Prerequisites

1. **Build release version:**
   ```powershell
   cargo build --release --features ffmpeg
   ```

2. **Close resource-intensive applications** (browsers, games, etc.)

3. **Open Task Manager:**
   - Press `Ctrl+Shift+Esc`
   - Go to **Details** tab
   - Sort by CPU column
   - Filter to show `liteclip-replay.exe`

4. **Optional: Enable debug logging:**
   ```powershell
   $env:RUST_LOG = "debug,liteclip_core=trace"
   cargo run --release --features ffmpeg
   ```

## Test 1: VAL-GUI-001 - CPU Usage at Idle (<0.1%)

**Steps:**
1. Start application
2. Wait 5 seconds for initialization
3. Close any open windows (settings, gallery)
4. Wait 3 seconds for dormancy activation
5. Observe Task Manager CPU column for 10 seconds
6. Record average CPU value

**Pass Criteria:** Average CPU < 0.1% over 10-second observation

**Evidence:** Screenshot of Task Manager showing CPU value

---

## Test 2: VAL-GUI-002 - Toast Notification Response (<100ms)

**Steps:**
1. Ensure application is idle (no windows visible)
2. Right-click tray icon → **Save Clip**
3. Observe toast appearance timing
4. Use video recording (OBS/Game Bar) if available

**Pass Criteria:** Toast appears <100ms after click (human perception: "instant")

**Evidence:** Video recording or screenshot of toast

---

## Test 3: VAL-GUI-003 - Settings Window Opening (<200ms)

**Steps:**
1. Ensure application is idle
2. Right-click tray icon → **Settings**
3. Observe window appearance timing
4. Use stopwatch or video recording

**Pass Criteria:** Settings window fully visible <200ms after click

**Evidence:** Screenshot of settings window

---

## Test 4: VAL-GUI-004 - Gallery Window Opening (<200ms)

**Steps:**
1. Ensure application is idle
2. Press Gallery hotkey OR right-click tray → **Gallery**
3. Observe window appearance timing

**Pass Criteria:** Gallery window fully visible <200ms after trigger

**Evidence:** Screenshot of gallery window

---

## Test 5: VAL-GUI-005 - Dormancy Activation (<3s)

**Steps:**
1. Open settings window
2. Close settings window
3. Observe Task Manager CPU
4. Wait and observe CPU drop timing

**Pass Criteria:** CPU drops to <0.1% within 3 seconds after closing windows

**Evidence:** Timeline observation notes

---

## Test 6: VAL-GUI-006 - Wake-on-Message Latency

**Steps:**
1. Ensure application is idle/dormant
2. Trigger any GUI event (tray click, hotkey)
3. Observe response timing

**Pass Criteria:** Response feels instant, no perceptible lag

**Evidence:** Debug logs with timestamps if enabled

---

## Test 7: VAL-GUI-007 - Memory Stability (<1MB growth)

**Steps:**
1. Record initial memory in Task Manager
2. Keep application idle for 5 minutes
3. Record final memory
4. Calculate difference

**Pass Criteria:** Memory growth <1MB over 5-minute period

**Evidence:** Memory values at T=0 and T=5min

---

## Test 8: VAL-GUI-008 - Hotkey Response from Dormant (<100ms)

**Steps:**
1. Ensure application is dormant (wait 5s idle)
2. Press Save Clip hotkey
3. Observe toast/response timing

**Pass Criteria:** Toast appears <100ms after hotkey press

**Evidence:** Video recording or observation notes

---

## Test 9: VAL-GUI-009 - GUI Idle During Recording

**Steps:**
1. Start recording (tray → Toggle Recording)
2. Close all windows
3. Wait 5 seconds for GUI dormancy
4. Observe GUI thread CPU vs recording thread CPU in Process Explorer

**Pass Criteria:** GUI thread CPU <0.5% while recording active

**Evidence:** Per-thread CPU screenshot

---

## Regression Tests (VAL-REG-*)

### VAL-REG-001: Recording Pipeline CPU Unchanged
- Compare recording thread CPU to baseline (before changes)
- Should be within ±5%

### VAL-REG-002: Hotkey Response Unchanged
- All hotkeys work correctly (Save, Toggle, Gallery)

### VAL-REG-003: Tray Icon Responsive
- Tray icon appears, menu opens quickly
- All menu items functional

### VAL-REG-004: Clip Save While Dormant
- Start recording, minimize GUI, save clip
- Verify clip file valid (play in VLC/mpv)

### VAL-REG-005: Memory Not Increased
- Compare total memory to baseline
- Should be within ±10MB

---

## Quick Validation Checklist

| Test | Action | Expected | Result |
|------|--------|----------|--------|
| CPU idle | Wait 10s idle | <0.1% | [ ] |
| Toast | Tray → Save Clip | <100ms | [ ] |
| Settings | Tray → Settings | <200ms | [ ] |
| Gallery | Hotkey/Tray | <200ms | [ ] |
| Dormancy | Close windows | <3s | [ ] |
| Hotkey | Press Save hotkey | <100ms | [ ] |
| Memory | 5 min idle | <1MB growth | [ ] |
| Recording | Record + idle GUI | <0.5% GUI | [ ] |

---

## Troubleshooting

**CPU still elevated (>0.1%):**
- Check if toast is still visible (animation running)
- Check if debug logging is enabled (extra log processing)
- Check for other applications competing for CPU

**Windows slow to open:**
- May be first-time window creation (subsequent opens faster)
- Check system CPU load

**Hotkey doesn't respond:**
- Hotkey registration issue (platform thread independent)
- Check hotkey configuration in Settings
