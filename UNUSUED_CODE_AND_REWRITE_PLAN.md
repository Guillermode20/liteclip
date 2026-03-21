# Unused Code and Rust Rewrite Plan

This plan turns the code-review findings into a practical cleanup sequence.
It focuses on two goals:

1. Remove dead or legacy code that is no longer used.
2. Rewrite retained code in a more idiomatic Rust style where the current shape is overly imperative or duplicated.

## Scope

The review targeted:

- `src/` desktop shell code
- `crates/liteclip-core/src/` engine code

The plan assumes the current behavior should stay unchanged unless a cleanup explicitly replaces an obsolete path.

## Priority 0: Confirm Before Deleting

These items look unused in the current tree, but should be confirmed once more before deletion if you want maximum safety.

- [`crates/liteclip-core/src/output/types.rs`](crates/liteclip-core/src/output/types.rs)
  - `Muxer::new`
  - `Muxer::write_video_packet`
  - `Muxer::write_audio_packet`
  - `Muxer::finalize`
  - private fields `config` and `stub_mode`
  - Rationale: the repo only appears to use `Muxer::mux_clip`, not the packet-by-packet API.

- [`src/gui/gallery/decode_pipeline/mod.rs`](src/gui/gallery/decode_pipeline/mod.rs)
  - `queue_health`
  - `needs_quality_reduction`
  - Rationale: no in-repo call sites were found.

- [`src/gui/gallery/decode_pipeline/frame_pool.rs`](src/gui/gallery/decode_pipeline/frame_pool.rs)
  - `FramePool::release`
  - Rationale: no in-repo call sites were found, and the current `rgba_frame_to_image_pooled` flow appears to consume the buffer without returning it to the pool.

- [`src/gui/gallery/browser.rs`](src/gui/gallery/browser.rs)
  - `gather_selected_entries`
  - Rationale: selection gathering is already implemented inline in the browser renderer.

- [`src/main.rs`](src/main.rs)
  - `hotkey_config_from_config`
  - Rationale: it is only `HotkeyConfig::from(config)`.

- [`src/gui/gallery.rs`](src/gui/gallery.rs)
  - `BROWSER_DELETE_HOLD_SECS`
  - `ThumbnailStrip::duration_secs`
  - `estimate_export_bitrates_from_editor` parameter `_fps`
  - Rationale: these are currently unused or only exist as passive placeholders.

- [`crates/liteclip-core/src/encode/ffmpeg/amf.rs`](crates/liteclip-core/src/encode/ffmpeg/amf.rs)
  - `create_d3d11_hardware_context`
  - Rationale: the current live AMF path uses the newer device-sharing helper instead.

- [`crates/liteclip-core/src/encode/ffmpeg/options.rs`](crates/liteclip-core/src/encode/ffmpeg/options.rs)
  - `hardware_frame_sw_format`
  - Rationale: appears to be kept only to support the older AMF path above.

## Priority 1: Remove Dead Code

These should be removed first if you want the largest codebase simplification with the least behavioral risk.

### 1. Remove the legacy packet-by-packet muxer surface

File: [`crates/liteclip-core/src/output/types.rs`](crates/liteclip-core/src/output/types.rs)

Actions:

- Delete the obsolete stateful methods from `Muxer`.
- Remove the now-unused fields if the type can be reduced to a pure config holder or removed entirely.
- Keep `Muxer::mux_clip` as the single active muxing entry point.
- If the legacy API is still needed for future work, move it behind an explicit `legacy-muxer` feature flag instead of leaving it in the default build.

Why this is first:

- It is the clearest unused surface in the repo.
- It adds maintenance cost without contributing to the current save flow.

### 2. Remove dead decode-pipeline helpers

Files:

- [`src/gui/gallery/decode_pipeline/mod.rs`](src/gui/gallery/decode_pipeline/mod.rs)
- [`src/gui/gallery/decode_pipeline/frame_pool.rs`](src/gui/gallery/decode_pipeline/frame_pool.rs)

Actions:

- Delete `queue_health` and `needs_quality_reduction` if no caller is added.
- Either remove `FramePool::release` or wire it into actual pooled-buffer reuse.
- If the pool is retained, redesign the image conversion path so the buffer lifecycle is explicit and returns to the pool.

Why this matters:

- The current `FramePool` shape suggests reuse, but the ownership flow does not clearly support it.
- Dead health helpers make the decoder harder to reason about.

### 3. Remove gallery shell wrappers that only duplicate a simpler expression

Files:

- [`src/gui/gallery/browser.rs`](src/gui/gallery/browser.rs)
- [`src/main.rs`](src/main.rs)

Actions:

- Delete `gather_selected_entries`.
- Inline `HotkeyConfig::from(config)` in `main.rs` unless you want the wrapper for readability in one location only.

Why this matters:

- These wrappers are not harmful, but they add indirection with no behavior.

### 4. Remove stale constants and passive fields

File: [`src/gui/gallery.rs`](src/gui/gallery.rs)

Actions:

- Remove `BROWSER_DELETE_HOLD_SECS` if no delete-hold UX uses it.
- Remove `ThumbnailStrip::duration_secs` if the strip never reads it.
- Remove `_fps` from `estimate_export_bitrates_from_editor` if bitrate estimation does not use it.

Why this matters:

- These are classic “almost used” leftovers that accumulate confusion during future edits.

## Priority 2: Rewrite for Rust Idioms

These are not strictly dead code, but they are the best candidates for a more idiomatic Rust rewrite.

### 1. Consolidate output-path generation

Files:

- [`crates/liteclip-core/src/output/functions.rs`](crates/liteclip-core/src/output/functions.rs)
- [`crates/liteclip-core/src/app/clip.rs`](crates/liteclip-core/src/app/clip.rs)

Actions:

- Move the shared path-generation logic into one canonical helper.
- Reuse that helper from the clip manager instead of keeping a second local implementation.
- Prefer a single source of truth for output directory rules, timestamp naming, and game-folder handling.

Suggested shape:

- Keep the reusable logic in `output::functions::generate_output_path`.
- Make `ClipManager::generate_output_path` a thin wrapper only if the app needs different folder rules.
- If the app rules are identical, delete the private helper and call the shared function directly.

### 2. Replace retry loops with iterator-driven candidate selection

File: [`src/gui/gallery.rs`](src/gui/gallery.rs)

Target:

- `build_clipped_output_path`

Actions:

- Replace the manual `for attempt in 0..1000` loop with a candidate iterator.
- Use a small helper that yields filenames until a free path is found.
- Keep the timestamp fallback as the final candidate.

Why this is better:

- It reduces nesting.
- It makes the intent more obvious.
- It is easier to test in isolation.

### 3. Simplify thumbnail-strip generation and parsing

File: [`src/gui/gallery.rs`](src/gui/gallery.rs)

Target:

- `generate_thumbnail_strip_frames`

Actions:

- Split command construction, frame extraction, and JPEG boundary parsing into smaller helpers.
- Replace byte-at-a-time parsing with a buffered parser if possible.
- Treat the FFmpeg subprocess as a boundary and keep the parsing logic explicit and testable.

Why this is better:

- The current function mixes process spawning, stream parsing, fallback logic, and sample interpolation in one block.
- Rust code reads better when ownership and I/O boundaries are clear.

### 4. Clean up legacy AMF helper shape

Files:

- [`crates/liteclip-core/src/encode/ffmpeg/amf.rs`](crates/liteclip-core/src/encode/ffmpeg/amf.rs)
- [`crates/liteclip-core/src/encode/ffmpeg/options.rs`](crates/liteclip-core/src/encode/ffmpeg/options.rs)

Actions:

- Remove the older `create_d3d11_hardware_context` path if it is no longer used.
- Remove `hardware_frame_sw_format` if nothing else needs the abstraction.
- Keep the AMF implementation centered around the active device-sharing path.

Why this is better:

- It removes API drift inside the encoder backend.
- It reduces the number of helpers future maintainers have to mentally reconcile.

### 5. Rework the gallery selection logic around the set as source of truth

File: [`src/gui/gallery/browser.rs`](src/gui/gallery/browser.rs)

Actions:

- Keep `selected_videos` as the canonical selection state.
- Express selection collection as an iterator chain where needed.
- Avoid separate “gather” helpers unless they are reused in multiple places.

Suggested style:

- `app.selected_videos.iter().filter_map(...)`
- `collect::<Vec<_>>()`

### 6. Make the frame pool lifecycle explicit or remove it

Files:

- [`src/gui/gallery/decode_pipeline/frame_pool.rs`](src/gui/gallery/decode_pipeline/frame_pool.rs)
- [`src/gui/gallery/decode_pipeline/mod.rs`](src/gui/gallery/decode_pipeline/mod.rs)

Actions:

- If pooling is useful, redesign `rgba_frame_to_image_pooled` so ownership returns to the pool after the image is no longer needed.
- If pooling does not materially help, delete the pool entirely and use straightforward allocation with `Vec<u8>`.

Why this is important:

- Half-implemented pooling is often worse than no pooling because it looks optimized while still allocating.

## Priority 3: Cleanup and Validation

After the code changes, do a validation pass.

### Validation steps

1. Run `cargo test --all-features`.
2. Run `cargo clippy --all-targets --all-features` again.
3. Verify the gallery still opens, scans videos, and deletes clips correctly.
4. Verify clip saving still works for:
   - video-only clips
   - clips with system audio
   - webcam companion clips
5. Verify the FFmpeg backend path used in `crates/liteclip-core/src/output/saver.rs` still produces the same output filenames and folders.

### What to watch for

- Any behavior change in output folder naming.
- Any loss of compatibility with existing gallery cache files.
- Any regression in the clip save path where `Muxer::mux_clip` is the only remaining muxing entry point.

## Recommended Order

1. Remove the dead helpers that have no callers.
2. Remove the legacy `Muxer` packet-by-packet API.
3. Delete stale fields/constants.
4. Consolidate duplicated output-path logic.
5. Simplify the gallery path generation and thumbnail parsing.
6. Reassess the frame pool and either make it real or remove it.

## Expected Result

After this cleanup, the codebase should have:

- fewer dead branches and fewer feature-era leftovers,
- a smaller public API surface,
- a clearer separation between active code and compatibility shims,
- and less imperative glue code in the GUI gallery paths.
