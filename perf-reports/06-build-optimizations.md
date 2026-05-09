# Build Optimization Analysis

**Date**: 2026-05-09
**Analysis Scope**: Cargo build configuration, profile settings, feature flags, dependency management, LTO strategy, build script overhead, benchmark configuration, and CPU-specific optimizations.

---

## Summary

LiteClip's release build configuration is well-optimized for final binary size and runtime performance (`codegen-units=1`, `lto="fat"`, `panic="abort"`, `strip=true`), but there are several missed opportunities for both **developer iteration speed** and **runtime CPU-specific optimizations**. Key gaps include: no `target-cpu=native` for release builds (leaving ~10–20% potential SIMD/microarchitecture perf on the table), duplicated `windows` crate feature lists across workspace members, no dev-profile LTO override to speed up debug builds, build script DLL copy overhead on every rebuild, weak GUI benchmarks, and missing incremental compilation tuning for development workflows.

**Overall build health**: Good for ship-ready releases. Poor for developer iteration speed. Several low-effort changes would meaningfully improve both.

---

## Hot Paths (Build)

| # | Path | Impact | Description |
|---|------|--------|-------------|
| 1 | `cargo build --release` | ~3–5 min | Full fat LTO + codegen-units=1 compilation |
| 2 | `cargo build` (debug) | ~30–60s | Incremental with `nnnoiseless` @ opt-level=3 |
| 3 | `build.rs` execution | ~200–500ms | FFmpeg DLL copy + icon generation every build |
| 4 | `cargo test --release` | ~5–8 min | Compiles release + runs criterion benches |
| 5 | FFmpeg link step | ~30–60s | `ffmpeg-next` statically links ~500K+ lines of C |

---

## Findings

### F-01: Missing `target-cpu=native` in Release Profile

| Field | Value |
|-------|-------|
| **Location** | `Cargo.toml` root — `[profile.release]` section |
| **Severity** | Medium |
| **Description** | Release builds compile for the baseline x86-64-v1 target (generic x64). No CPU-specific instruction set extensions (AVX2, AVX-512, BMI1/2) are enabled, leaving 10–20% runtime performance untapped on modern CPUs. |
| **Code** | `Cargo.toml` lines 36–39: `[profile.release]` has no `target-cpu` setting |
| **Why** | LiteClip is a native Windows screen recorder shipping to end-user machines. The capture pipeline does heavy pixel format conversion (BGRA→NV12), audio mixing, and JPEG/HEVC software encoding fallback — all of which benefit from SIMD. |
| **Recommendation** | Add `target-cpu = "native"` to `[profile.release]`. This enables the compiler to use all available CPU features on the build machine (AVX2, FMA, BMI, MOVBE, etc.). For shipped binaries, consider `target-cpu = "x86-64-v3"` (AVX2 baseline, ~2013+ CPUs) if you want a portable-but-fast middle ground, or use a CI runner with a specific CPU target. **Caveat**: `target-cpu = "native"` makes binaries non-portable to older CPUs — ensure CI builder matches minimum supported target. |

---

### F-02: No ~/.cargo/config.toml for CI/Developer Overrides

| Field | Value |
|-------|-------|
| **Location** | `.cargo/config.toml` (project level) |
| **Severity** | Low |
| **Description** | The project `.cargo/config.toml` only sets environment variables for FFMPEG_DIR and LLVM bindgen. No codegen/profile overrides for CI or development. |
| **Code** | `.cargo/config.toml` |
| **Why** | Developers often want different profiles (e.g., faster debug builds, CI with LTO disabled for speed). Without a config override mechanism, every developer must manually edit Cargo.toml. |
| **Recommendation** | Create `Cargo.toml` `[profile.dev]` overrides to disable LTO in dev (already implicit), but consider adding `[profile.dev]` with `incremental = true` (already default) as an explicit reminder. For CI, the `CARGO_PROFILE_RELEASE_LTO=thin` env var can be documented. |

---

### F-03: Dev Profile Lacks Per-Crate Optimization for Hot Crates Beyond `nnnoiseless`

| Field | Value |
|-------|-------|
| **Location** | `Cargo.toml` — `[profile.dev.package.nnnoiseless]` |
| **Severity** | Low |
| **Description** | Only `nnnoiseless` gets `opt-level=3` in debug builds. Other hot crates (`liteclip-core` itself — specifically the ring buffer, DXGI capture, audio mixer, and encoder) default to `opt-level=0` in dev, making debug builds unrepresentatively slow for those hot paths. |
| **Code** | `Cargo.toml` line 35: `[profile.dev.package.nnnoiseless]\nopt-level = 3` |
| **Why** | Developers debugging performance issues locally get a misleading picture when hot code paths are unoptimized. For example, the ring buffer and audio mixer are microbenchmarked but unoptimized in debug builds. |
| **Recommendation** | Add `[profile.dev.package.liteclip-core]` with `opt-level = 2` (or at least `1`) to get meaningful debug performance. Also consider `opt-level = 1` for `crossbeam`, `parking_lot`, `ffmpeg-next` if benchmarks show them as hot. |

---

### F-04: Duplicated `windows` Crate Feature Lists Across Workspace Members

| Field | Value |
|-------|-------|
| **Location** | Both `Cargo.toml` (root) and `crates/liteclip-core/Cargo.toml` |
| **Severity** | Low-Medium |
| **Description** | The identical 24-feature `windows = { version = "0.58", features = [...]}` block appears verbatim in both `Cargo.toml` files. The root crate may not need all features (e.g., `Win32_Devices_FunctionDiscovery` might only be used in liteclip-core). This increases compile time slightly due to feature resolution overhead. |
| **Code** | `Cargo.toml` lines 16–18 AND `crates/liteclip-core/Cargo.toml` lines 37–39 |
| **Why** | Unnecessary duplication. If root only needs a subset (e.g., `Win32_Media`, `Win32_Foundation`, `Win32_UI_WindowsAndMessaging` for tray/hotkeys), the smaller feature set would reduce the Windows metadata the compiler tokenizes. |
| **Recommendation** | Audit which `windows` features each crate actually uses via `cargo tree -e features -p windows`. Trim the root crate's feature list to only what `src/` directly uses. Currently `Win32_System_Performance`, `Win32_System_Registry`, `Win32_System_ProcessStatus`, `Win32_Devices_FunctionDiscovery`, `Win32_Security`, `Win32_System_JobObjects` may only be needed by one crate. |

---

### F-05: `ureq` Dependency in Root Without Conditional Features

| Field | Value |
|-------|-------|
| **Location** | `Cargo.toml` root |
| **Severity** | Low |
| **Description** | `ureq = { version = "3", features = ["json"] }` is an unconditional dependency in the root crate (for the update checker). It brings in TLS, JSON parsing, and HTTP stack. If the update checker is a minor feature, this adds ~80+ dependencies to the tree for every build. |
| **Code** | `Cargo.toml` line 31: `ureq = { version = "3", features = ["json"] }` |
| **Why** | The commented-out `# ureq = { version = "3", optional = true }` in liteclip-core suggests awareness that ureq should be optional. It still adds to compile time for every debug build. |
| **Recommendation** | Make `ureq` optional behind a feature gate (e.g., `updater` or `auto-update`) in the root crate. Default to `off` for dev builds, `on` for release builds. |

---

### F-06: Build Script DLL Copy on Every Build is Wasteful

| Field | Value |
|-------|-------|
| **Location** | `build.rs` — `copy_runtime_dlls()` function |
| **Severity** | Medium |
| **Description** | Every `cargo build` invocation copies all FFmpeg DLLs from `ffmpeg_dev/sdk/bin/` to `target/<profile>/` and `target/<profile>/deps/`. This is ~20–40 MB of DLL files copied every time, even when nothing changed. The script uses `cargo:rerun-if-changed=` for the directory and `FFMPEG_DIR` env var, but it checks existence rather than modification time. |
| **Code** | `build.rs` lines 14–90 |
| **Why** | Developers rebuilding frequently (e.g., `cargo check` → `cargo build` → `cargo test`) pay for this I/O on every invocation. The `// Skip if destination exists` check helps, but the `fs::read_dir` and copy operations still execute. |
| **Recommendation** | Use `cargo:rerun-if-changed=` on specific DLL paths rather than the whole directory. Or, consider using a symbolic link (junction) on Windows instead of copying. At minimum, add a hash check: compute a hash of the source dir files, only re-copy when it changes. |

---

### F-07: Fat LTO Has Diminishing Returns for This Project

| Field | Value |
|-------|-------|
| **Location** | `Cargo.toml` — `[profile.release]` |
| **Severity** | Low-Medium |
| **Description** | `lto = "fat"` performs full cross-crate LTO across all dependencies. For a project of this size (~20–30 workspace + ~150–200 dependency crates), fat LTO adds 30–60s of link time over thin LTO, and the runtime benefit is typically <2% over thin LTO. |
| **Code** | `Cargo.toml` line 38: `lto = "fat"` |
| **Why** | Fat LTO is best for very large monoliths or when binary size is critical. LiteClip is moderate-sized. Thin LTO provides 90%+ of the optimization benefit with ~2–3x faster linking. |
| **Recommendation** | Switch to `lto = "thin"`. Profile the binary size difference — expect <1% size increase and <1% runtime regression. If binary size is a hard requirement, keep fat LTO but this should be measured rather than assumed necessary. |

---

### F-08: No Dev Profile LTO Override for Build Script DLL Copy Hot-Reload

| Field | Value |
|-------|-------|
| **Location** | `Cargo.toml` — missing `[profile.dev]` section |
| **Severity** | Low |
| **Description** | There is no explicit `[profile.dev]` section, so debug builds use defaults (LTO = off, codegen-units = 16). This is fine, but there's no `lto = "thin"` for dev builds to speed up the edit-compile-test loop, and no explicit `incremental = true` toggling. |
| **Code** | Missing from `Cargo.toml` |
| **Why** | Debug builds already compile fast enough (~30-60s), but adding explicit settings makes the configuration self-documenting. More importantly, turning on `lto = "thin"` for dev builds with the DLL copy overhead can reduce false `STATUS_DLL_NOT_FOUND` errors during rapid iteration. |
| **Recommendation** | Add an explicit `[profile.dev]` section with a comment documenting settings. Consider `lto = "off"` (already default) explicitly noted. |

---

### F-09: GUI Benchmarks Are Stubs, Not Real Benchmarks

| Field | Value |
|-------|-------|
| **Location** | `benches/gui_interactions.rs` |
| **Severity** | Medium |
| **Description** | The workspace-level GUI benchmarks (`benches/gui_interactions.rs`) do not actually benchmark real GUI code. `bench_gui_state_transitions` sums integers 0..99, and `gui_event_processing` sums a vector. These are CPU no-ops that measure nothing about LiteClip's actual GUI performance. |
| **Code** | `benches/gui_interactions.rs` lines 8–33 |
| **Why** | These benchmarks run as real criterion benchmarks, producing reports that look meaningful but are actually synthetic noise. They waste CI time and give no actionable data. |
| **Recommendation** | Either remove the `guu_interactions` benchmark group entirely, or replace them with real benchmarks: measure egui frame render time (e.g., `egui::Context::run` on a test UI), measure gallery thumbnail decode latency, or measure settings page config serialization render impact. The stubs as-is are actively harmful. |

---

### F-10: `image` Crate with `ico` Feature in Both Main and Build Dependencies

| Field | Value |
|-------|-------|
| **Location** | `Cargo.toml` — `[dependencies]` and `[build-dependencies]` |
| **Severity** | Low |
| **Description** | The `image` crate appears in both `[dependencies]` (features: jpeg, png, ico) and `[build-dependencies]` (features: ico only). The `ico` feature in `[dependencies]` exists solely for the gallery thumbnail ICO handling, while `[build-dependencies]` uses image + ico to generate the application icon. The `ico` feature in `[dependencies]` might not be needed by runtime code if icons are only generated at build time. |
| **Code** | `Cargo.toml` line 22 (image dep) and line 42 (build-dep image dep) |
| **Why** | Feature redundancy. If runtime code doesn't use `image::codecs::ico`, the `ico` feature can be removed from `[dependencies]` reducing crate compilation slightly. |
| **Recommendation** | Grep for `ico` usage in `src/*.rs`. If only `build.rs` uses ICO encoding, remove `ico` from `[dependencies]` and keep only in `[build-dependencies]`. |

---

### F-11: Missing Benchmark for Capture Pipeline Latency

| Field | Value |
|-------|-------|
| **Location** | `crates/liteclip-core/benches/*.rs` and `benches/*.rs` |
| **Severity** | Low-Medium |
| **Description** | There are benchmarks for ring buffer push/snapshot, config serialization, and audio mixer creation — but **no benchmark for the critical capture-to-encode latency path**. The DXGI capture + pixel format conversion + encoder submission is the most performance-sensitive pipeline in the entire application. |
| **Code** | Missing benchmark file |
| **Why** | Capture latency is the #1 metric for a screen recorder. Without a benchmark measuring end-to-end frame delivery time (from DXGI Present → encoded packet available), regressions go undetected until users report stutter. |
| **Recommendation** | Add a benchmark that: (1) creates mock D3D11 textures, (2) feeds them through the `CapturePipeline` or `DxgiCapture` + `Encoder` chain, (3) measures latency per frame. This could use `criterion` with `Bencher::iter_custom` for wall-clock measurement. Even a synthetic benchmark using pre-allocated CPU-side BGRA buffers would be useful. |

---

### F-12: `windows-core` Crate Is a Separate Dependency

| Field | Value |
|-------|-------|
| **Location** | `Cargo.toml` (both root and core) |
| **Severity** | Low |
| **Description** | Both Cargo.toml files list `windows-core = "0.58"` as an explicit dependency. This crate is automatically pulled in by `windows = "0.58"` anyway, so the explicit dependency is redundant. |
| **Code** | `Cargo.toml` line 21: `windows-core = "0.58"` |
| **Why** | Redundant dependencies add no compilation time (already resolved), but clutter the manifest and can cause version mismatches if not kept in sync. |
| **Recommendation** | Remove explicit `windows-core` dependency from both Cargo.toml files. Use `windows` re-exports instead (`windows::core::...`) or add a comment explaining why it's needed (e.g., for trait impls not available through `windows` crate re-exports). |

---

### F-13: `image` Crate 0.25 Dependency with `jpeg` + `png` Features for Gallery Thumbnails

| Field | Value |
|-------|-------|
| **Location** | Both `Cargo.toml` files |
| **Severity** | Low |
| **Description** | The `image` crate with `jpeg` and `png` decoders is included for gallery thumbnail loading. If the gallery uses `ffmpeg-next` for frame extraction (which it does — it decodes frames from MP4), the `image` crate JPEG/PNG decoder is actually used for *metadata/cover art* scenarios, not primary video rendering. Its weight (~40 transitive deps including crossbeam-channel) may be disproportionate to its usage. |
| **Code** | `Cargo.toml` line 22 `image = { version = "0.25", default-features = false, features = ["jpeg", "png", "ico"] }` |
| **Why** | Every dependency adds to compile time. If image loading is only used for the tray icon (PNG) and build-time icon generation, the `jpeg` feature can be dropped. |
| **Recommendation** | Audit `image` usage in source. If JPEG decoding is never used at runtime, drop the `jpeg` feature. If ICO is only used in `build.rs`, drop it from main dependencies. |

---

### F-14: `crossbeam` 0.8 Dependency with Only Channel Usage

| Field | Value |
|-------|-------|
| **Location** | Both `Cargo.toml` files |
| **Severity** | Low |
| **Description** | `crossbeam = "0.8"` is a dependency bringing the entire crossbeam suite (epoch, deque, queue, channel, utils). LiteClip likely only uses `crossbeam::channel`. The full crossbeam crate compiles many modules that may be unused. |
| **Code** | `Cargo.toml` line 11: `crossbeam = "0.8"` |
| **Why** | Even if unused modules are optimized out by dead code elimination in release, the **compilation** time for debug builds includes all modules. Switching to `crossbeam-channel` would reduce debug compile time. |
| **Recommendation** | Replace `crossbeam = "0.8"` with `crossbeam-channel = "0.8"` if `crossbeam::channel` is the only feature used. Grep for `crossbeam::` usage patterns to confirm. |

---

## Scoring

| Area | Score (1–10) | Notes |
|------|-------------|-------|
| **Release Profile** | 7/10 | Good foundation (LTO, panic=abort, strip, codegen-units=1). Missing `target-cpu=native`. |
| **Debug/Dev Profile** | 4/10 | No explicit dev profile tuning. Only `nnnoiseless` optimized in dev. |
| **Feature Flags** | 6/10 | Commented-out crates cleaned up. `ureq` unconditional. Duplicated windows features. |
| **Dependency Management** | 5/10 | Several redundant/over-broad deps (windows-core, crossbeam, image features). |
| **LTO Strategy** | 6/10 | Fat LTO may be overkill for project size. Thin LTO would save link time. |
| **Build Script** | 5/10 | DLL copy is naive (full copy every build). Works but wastes I/O. |
| **Benchmarks** | 4/10 | Ring buffer and config benchmarks are good. GUI benchmarks are stubs. Missing capture latency bench. |
| **CPU Optimizations** | 3/10 | No target-cpu, no explicit SIMD for audio/pixel conversion. Heavy reliance on ffmpeg-next for optimizations. |
| **Documentation** | 6/10 | AGENTS.md documents build commands well. Missing build optimization rationale documentation. |
| **CI Configuration** | 5/10 | GitHub Actions present. No CI-specific profile overrides documented. |

**Overall**: 5.1/10 — Functional but has multiple low-effort improvements available.

---

## Recommendations Priority

| Priority | Action | Effort | Impact |
|----------|--------|--------|--------|
| P0 | Add `target-cpu = "native"` to `[profile.release]` | 1 line | High — 10–20% runtime perf on modern CPUs |
| P1 | Fix GUI benchmarks (remove stubs or replace with real benches) | ~2h | Medium — CI accuracy, no more fake metrics |
| P1 | Switch to `lto = "thin"` for release builds | 1 line | Medium — 30–60s faster release links |
| P2 | Make `ureq` optional behind feature flag | 15 min | Low — removes ~80 deps from debug tree |
| P2 | Audit and deduplicate `windows` crate features per workspace member | 1–2h | Low-Medium — slightly faster metadata build |
| P2 | Add capture latency benchmark | 4–8h | Medium — catches pipeline regressions |
| P3 | Optimize build.rs DLL copy with hash-based change detection | 2h | Low — saves ~200ms per build |
| P3 | Replace `crossbeam` with `crossbeam-channel` | 15 min | Low — slightly faster debug compile |
| P3 | Remove explicit `windows-core` dependency | 5 min | Low — manifest cleanup |
| P3 | Audit `image` crate features for unused codecs | 30 min | Low — marginally faster debug compile |
