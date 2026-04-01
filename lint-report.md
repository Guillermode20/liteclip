# LiteClip Rust Linting & Analysis Report
Generated: 2026-04-01

## Summary

| Tool | Status | Findings |
|------|--------|----------|
| Clippy (pedantic + nursery) | WARNINGS | See detailed breakdown below |
| Cargo Audit | 12 ALLOWED WARNINGS | Unmaintained/unsound dependencies |
| Cargo Udeps | PASS | All dependencies used |
| Cargo Fmt | PASS | Code properly formatted |

---

## 1. Clippy (pedantic + nursery) Findings

### Categories of Warnings

#### A. Documentation - Missing Backticks (`doc_markdown`)
**Count: ~15 warnings**

These are in documentation comments where terms like `FFmpeg`, `LiteClip`, `RecordingPipeline`, `STATUS_DLL_NOT_FOUND` should be wrapped in backticks.

**Affected files:**
- `crates/liteclip-core/build.rs:2,19` - FFmpeg references
- `build.rs:15,16,19` - FFmpeg, STATUS_DLL_NOT_FOUND
- `crates/liteclip-core/src/lib.rs:1,3,13,25,39,75,80` - LiteClip, FFmpeg
- `crates/liteclip-core/src/app/mod.rs:4` - LiteClip
- `crates/liteclip-core/src/app/pipeline/mod.rs:11` - FFmpeg
- `crates/liteclip-core/src/app/pipeline/manager.rs:53,75,77` - RecordingPipeline, FFmpeg
- `crates/liteclip-core/src/app/state.rs:11` - LiteClip

**Fix:** Wrap these terms in backticks in doc comments:
```rust
// Before: //! LiteClip core — Windows screen capture
// After:  //! `LiteClip` core — Windows screen capture
```

#### B. Similar Variable Names (`similar_names`)
**Count: ~20 warnings**

Variables with names that differ only slightly, making them hard to distinguish:

**Affected files:**
- `crates/liteclip-core/src/buffer/ring/spmc_ring.rs:1173-1174`
  - `first_nal_is_vps` vs `first_nal_is_sps`
- `crates/liteclip-core/src/encode/sw_encoder.rs:62-73`
  - `src_y_clamped`, `src_x_clamped`, `src_y0`, `src_x0`, `src_x1`
- `crates/liteclip-core/src/output/mp4.rs:675-678,793-796`
  - `planar_i16_left/right` vs `planar_f32_left/right`
- `crates/liteclip-core/src/output/sdk_export.rs:180-181,256-261,257-259,270-271,563-566,741-742,822-826,913-914,974-978,1018-1022,1045-1046`
  - `last_out_dts_by_stream` vs `last_out_pts_by_stream`
  - `output_pts_secs` vs `output_dts_secs`
  - `adjusted_pts` vs `adjusted_dts`
  - `fixed_pts` vs `fixed_dts`
  - `next_video_pts` vs `next_video_dts`
  - `next_audio_pts` vs `next_audio_dts`
- `crates/liteclip-core/src/output/sdk_ffmpeg_output.rs:193-198`
  - `decoder` vs `decoded`

**Note:** Many of these are intentional (pts/dts pairs, left/right channels). Consider adding `#[allow(clippy::similar_names)]` at the module level for files with many intentional similar names.

#### C. Missing `#[must_use]` Attributes
**Count: ~12 warnings**

Functions returning values that should not be ignored:

**Affected files:**
- `crates/liteclip-core/src/app/pipeline/audio.rs:91` - `is_running()`
- `crates/liteclip-core/src/app/pipeline/manager.rs:59,78,91,96,105` - `new()`, `with_defaults()`, `level_monitor()`, `lifecycle()`, `is_recording()`
- `crates/liteclip-core/src/app/state.rs:83,142,146,155,232,245` - `core_host()`, `save_context()`, `replay_buffer_stats()`, `is_recording()`, `config()`, `level_monitor()`

**Fix:** Add `#[must_use]` attribute before each function.

#### D. Missing `const fn`
**Count: ~6 warnings**

Functions that could be `const`:

**Affected files:**
- `crates/liteclip-core/src/app/pipeline/audio.rs:35` - `AudioCaptureHandle::new()`
- `crates/liteclip-core/src/app/pipeline/manager.rs:91,96,105` - `level_monitor()`, `lifecycle()`, `is_recording()`
- `crates/liteclip-core/src/app/state.rs:232,245` - `config()`, `level_monitor()`

**Fix:** Add `const` keyword to function declaration.

#### E. Missing `# Errors` Documentation
**Count: ~6 warnings**

Functions returning `Result` without documenting possible errors:

**Affected files:**
- `crates/liteclip-core/src/app/pipeline/audio.rs:132` - `start_audio_capture()`
- `crates/liteclip-core/src/app/pipeline/manager.rs:147,202` - `start()`, `stop()`
- `crates/liteclip-core/src/app/pipeline/video.rs:14` - `start_video_pipeline()`
- `crates/liteclip-core/src/app/state.rs:124` - `enforce_pipeline_health()`
- `crates/liteclip-core/src/benchmark_harness.rs:203` - `summarize_benchmark_suite()`

**Fix:** Add `# Errors` section to doc comments describing when errors occur.

#### F. Redundant `else` Blocks
**Count: 4 warnings**

**Affected file:**
- `crates/liteclip-core/src/capture/audio/mixer.rs:348,365,373,380`

**Fix:** Remove `else` blocks and move contents out (early return pattern).

#### G. Uninlined Format Args
**Count: ~4 warnings**

**Affected files:**
- `build.rs:117-120` - `println!` with `failed`
- `crates/liteclip-core/src/app/pipeline/manager.rs:282,293` - `format!` with `reason`
- `crates/liteclip-core/src/app/state.rs:210-213` - `anyhow::anyhow!` with `e`

**Fix:** Use inline format args:
```rust
// Before: format!("Encoder fatal: {}", reason)
// After:  format!("Encoder fatal: {reason}")
```

#### H. Unreadable Literals
**Count: 2 warnings**

**Affected file:**
- `crates/liteclip-core/src/capture/dxgi/capture.rs:298,345` - `0x10000000u32`

**Fix:** Add underscores: `0x1000_0000_u32`

#### I. Match Same Arms
**Count: 1 warning**

**Affected file:**
- `crates/liteclip-core/src/app/clip.rs:87-88`
  - `Resolution::Native` and `Resolution::P1080` both return `(1920, 1080)`

**Fix:** Merge patterns:
```rust
crate::config::Resolution::Native | crate::config::Resolution::P1080 => (1920, 1080),
```

#### J. Cast Lossless
**Count: 2 warnings**

**Affected file:**
- `crates/liteclip-core/src/app/clip.rs:92,99`

**Fix:** Use `From` instead of `as`:
```rust
// Before: config.video.framerate as f64
// After:  f64::from(config.video.framerate)
```

#### K. Manual Let-Else / Single Match Else
**Count: 1 warning**

**Affected file:**
- `crates/liteclip-core/src/app/pipeline/audio.rs:173`

**Fix:** Use `let...else` pattern:
```rust
let Ok(packet) = recv_result else {
    debug!(...);
    break;
};
```

#### L. Option If-Let-Else
**Count: 1 warning**

**Affected file:**
- `build.rs:33-37`

**Fix:** Use `map_or_else`:
```rust
std::env::var("FFMPEG_DIR").map_or_else(
    |_| manifest_dir.join("ffmpeg_dev").join("sdk").join("bin"),
    |dir| PathBuf::from(dir).join("bin"),
)
```

#### M. Manual Is-Variant-And
**Count: 2 warnings**

**Affected files:**
- `crates/liteclip-core/build.rs:52-56`
- `build.rs:74-78`

**Fix:** Use `is_none_or`:
```rust
// Before: .map(|e| e.eq_ignore_ascii_case("dll")) != Some(true)
// After:  .is_none_or(|e| !e.eq_ignore_ascii_case("dll"))
```

#### N. Items After Statements
**Count: 1 warning**

**Affected file:**
- `crates/liteclip-core/src/app/pipeline/audio.rs:64`

**Fix:** Move `const JOIN_TIMEOUT` to the top of the scope.

---

## 2. Cargo Audit Findings

### Unmaintained Dependencies (10 crates)

All related to GTK3 bindings via `tray-icon` dependency chain:

| Crate | Version | Advisory | Path |
|-------|---------|----------|------|
| atk | 0.18.2 | RUSTSEC-2024-0413 | tray-icon → muda/libappindicator → gtk |
| atk-sys | 0.18.2 | RUSTSEC-2024-0416 | Same chain |
| gdk | 0.18.2 | RUSTSEC-2024-0412 | Same chain |
| gdk-sys | 0.18.2 | RUSTSEC-2024-0418 | Same chain |
| gtk | 0.18.2 | RUSTSEC-2024-0415 | Same chain |
| gtk-sys | 0.18.2 | RUSTSEC-2024-0420 | Same chain |
| gtk3-macros | 0.18.2 | RUSTSEC-2024-0419 | Same chain |
| paste | 1.0.15 | RUSTSEC-2024-0436 | eframe → wgpu → wgpu-hal → metal |
| proc-macro-error | 1.0.4 | RUSTSEC-2024-0370 | Same GTK chain |
| atty | 0.2.14 | RUSTSEC-2024-0375 | liteclip-core → nnnoiseless → clap |

### Unsound Dependencies (2 crates)

| Crate | Version | Advisory | Issue |
|-------|---------|----------|-------|
| atty | 0.2.14 | RUSTSEC-2021-0145 | Potential unaligned read |
| glib | 0.18.5 | RUSTSEC-2024-0429 | Unsoundness in VariantStrIter impls |

### Recommendations

1. **GTK3 bindings:** These come from `tray-icon` → `muda`/`libappindicator` → `gtk`. Since this is a Windows-only app, consider:
   - Checking if `tray-icon` has updated dependencies
   - Using `allow` in `.cargo/audit.toml` for these if they're Linux-only code paths

2. **atty:** Transitive via `nnnoiseless` → `clap`. Consider filing an issue with `nnnoiseless` to update `clap` or remove `atty` dependency.

3. **paste:** From `wgpu-hal` → `metal`. This is macOS-specific and won't affect Windows builds.

4. **glib:** Same GTK3 chain as above.

---

## 3. Cargo Udeps

**Result: PASS** - All dependencies are used.

No unused dependencies detected in either `liteclip` or `liteclip-core`.

---

## 4. Cargo Fmt

**Result: PASS** - Code is properly formatted.

No formatting issues found.

---

## Priority Recommendations

### High Priority (Fix These)
1. **Match same arms** (`app/clip.rs:87-88`) - Potential bug if Native should differ from P1080
2. **Cast lossless** (`app/clip.rs:92,99`) - Safer casting patterns
3. **Redundant else** (`capture/audio/mixer.rs`) - Cleaner code

### Medium Priority (Improve Code Quality)
1. **Uninlined format args** - Modern Rust style
2. **Unreadable literals** - Readability
3. **Manual let-else** - Modern pattern
4. **Missing #[must_use]** - API safety
5. **Missing # Errors docs** - Better documentation

### Low Priority (Style Preferences)
1. **Similar names** - Many are intentional (pts/dts pairs)
2. **Missing const fn** - Minor optimization
3. **Doc markdown** - Documentation formatting
4. **Items after statements** - Style preference

### Dependency Actions
1. Review `tray-icon` updates for GTK3 dependency chain
2. Consider adding `.cargo/audit.toml` to allow known unmaintained transitive deps
3. Monitor `nnnoiseless` for clap/atty updates
