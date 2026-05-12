//! Frame buffer pool for video decode pipeline.
//!
//! Manages a pool of `Vec<Color32>` buffers sized to the output resolution.
//! The scaler writes directly into these buffers, then they're handed to
//! `texture.set()` as a `ColorImage` — avoiding the intermediate `RgbaImage`
//! copy and the per-pixel `ColorImage::from_rgba_unmultiplied` conversion.
//!
//! The idea:
//!   1. Scaler writes RGBA bytes into a pool-backed `ColorImage` (zero-copy).
//!   2. `ColorImage` goes through the channel to the UI thread.
//!   3. `texture.set(ColorImage, options)` consumes it; GPU upload reads via
//!      `bytemuck::cast_slice` (also zero-cost reinterpret).
//!   4. The pool allocates new buffers as needed; trimming keeps memory bounded.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use egui::epaint::{Color32, ColorImage};

pub(super) const FRAME_POOL_SIZE: usize = 32;
pub(super) const FRAME_POOL_MAX_SIZE: usize = 48;

/// A pool of `Vec<Color32>` buffers sized to a fixed resolution.
///
/// The scaler writes directly into the backing storage of these buffers.
/// Each buffer can be "finalized" into a `ColorImage` for direct use with
/// `texture.set()`. When the `PooledColorImage` is dropped after upload,
/// the backing `Vec<Color32>` is returned to the pool.
pub(super) struct FramePool {
    buffers: Mutex<VecDeque<Vec<Color32>>>,
    pixel_count: usize,
    total_created: Mutex<usize>,
}

impl FramePool {
    pub(super) fn new(width: u32, height: u32, capacity: usize) -> Self {
        let pixel_count = (width as usize) * (height as usize);
        let buffers: VecDeque<Vec<Color32>> = (0..capacity)
            .map(|_| vec![Color32::BLACK; pixel_count])
            .collect();
        Self {
            buffers: Mutex::new(buffers),
            pixel_count,
            total_created: Mutex::new(capacity),
        }
    }

    /// Acquire a zero-initialized `PooledColorImage`.
    ///
    /// The backing `Vec<Color32>` is sourced from the pool when possible.
    /// The `raw_rgba_mut()` slice can be handed directly to the FFmpeg
    /// scaler for a zero-copy write.
    pub(super) fn acquire_color_image(
        self: &Arc<Self>,
        width: u32,
        height: u32,
    ) -> PooledColorImage {
        let mut guard: MutexGuard<'_, VecDeque<Vec<Color32>>> =
            self.buffers.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(mut buffer) = guard.pop_front() {
            // Reset any residual data (should already be black from release).
            let pixel_count = (width as usize) * (height as usize);
            if buffer.len() != pixel_count {
                buffer.resize(pixel_count, Color32::BLACK);
            }
            let w = width as usize;
            let h = height as usize;
            return PooledColorImage {
                color_image: Some(ColorImage {
                    size: [w, h],
                    source_size: egui::vec2(w as f32, h as f32),
                    pixels: buffer,
                }),
                pool: Arc::clone(self),
            };
        }

        // Pool exhausted — allocate fresh.
        let pixel_count = (width as usize) * (height as usize);
        let mut total = self
            .total_created
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        *total += 1;
        let w = width as usize;
        let h = height as usize;
        PooledColorImage {
            color_image: Some(ColorImage {
                size: [w, h],
                source_size: egui::vec2(w as f32, h as f32),
                pixels: vec![Color32::BLACK; pixel_count],
            }),
            pool: Arc::clone(self),
        }
    }

    /// Return a `Vec<Color32>` to the pool.
    fn release(&self, mut buffer: Vec<Color32>) {
        buffer.fill(Color32::BLACK);
        let mut guard: MutexGuard<'_, VecDeque<Vec<Color32>>> =
            self.buffers.lock().unwrap_or_else(PoisonError::into_inner);
        if guard.len() < FRAME_POOL_MAX_SIZE {
            guard.push_back(buffer);
        }
    }

    pub(super) fn trim_to(&self, target: usize) {
        let mut guard: MutexGuard<'_, VecDeque<Vec<Color32>>> =
            self.buffers.lock().unwrap_or_else(PoisonError::into_inner);
        while guard.len() > target {
            guard.pop_back();
        }
    }

    pub(super) fn clear(&self) {
        let mut guard: MutexGuard<'_, VecDeque<Vec<Color32>>> =
            self.buffers.lock().unwrap_or_else(PoisonError::into_inner);
        guard.clear();
    }
}

/// A `ColorImage` whose backing `Vec<Color32>` returns to the pool on drop.
///
/// The `ColorImage` is accessed via [`color_image()`] or extracted via
/// [`into_color_image()`] for use with `texture.set()`.
pub struct PooledColorImage {
    color_image: Option<ColorImage>,
    pool: Arc<FramePool>,
}

impl PooledColorImage {
    /// Get a mutable byte slice into the pixel buffer for the scaler to write into.
    ///
    /// Length is `width * height * 4`. Writing RGBA bytes here directly populates
    /// the `ColorImage`'s `Vec<Color32>`.
    ///
    /// # Safety
    ///
    /// The caller MUST write valid RGBA pixel data covering exactly this slice.
    /// Every pixel MUST be fully opaque (a == 255) since `Color32` uses
    /// premultiplied alpha internally and this function skips premultiplication.
    pub fn raw_rgba_mut(&mut self) -> &mut [u8] {
        let ci = self
            .color_image
            .as_mut()
            .expect("PooledColorImage already consumed");
        // SAFETY: Color32 is #[repr(C)] [u8; 4], so Vec<Color32> has the same
        // memory layout as Vec<u8> with 4× the element count. We reinterpret
        // the backing storage as a mutable byte slice for the scaler to write into.
        let len = ci.pixels.len() * 4;
        unsafe { std::slice::from_raw_parts_mut(ci.pixels.as_mut_ptr() as *mut u8, len) }
    }

    /// Consume the wrapper and return the inner `ColorImage`.
    ///
    /// The backing `Vec<Color32>` will **not** be returned to the pool
    /// (it's consumed by the caller, e.g. by `texture.set()`).
    pub fn into_color_image(mut self) -> ColorImage {
        self.color_image
            .take()
            .expect("PooledColorImage already consumed")
    }
}

impl Drop for PooledColorImage {
    fn drop(&mut self) {
        if let Some(ci) = self.color_image.take() {
            if ci.pixels.len() == self.pool.pixel_count {
                self.pool.release(ci.pixels);
            }
        }
    }
}
