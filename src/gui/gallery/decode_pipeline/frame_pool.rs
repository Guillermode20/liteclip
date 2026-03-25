use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

pub(super) const FRAME_POOL_SIZE: usize = 32;
pub(super) const FRAME_POOL_HARD_LIMIT: usize = 64;

pub(super) struct FramePool {
    buffers: Mutex<VecDeque<Vec<u8>>>,
    buffer_size: usize,
    total_created: Mutex<usize>,
}

impl FramePool {
    pub(super) fn new(width: u32, height: u32, capacity: usize) -> Self {
        let buffer_size = (width as usize) * (height as usize) * 4;
        let buffers: VecDeque<Vec<u8>> = (0..capacity).map(|_| vec![0u8; buffer_size]).collect();
        Self {
            buffers: Mutex::new(buffers),
            buffer_size,
            total_created: Mutex::new(capacity),
        }
    }

    pub(super) fn acquire(self: &Arc<Self>) -> PooledBuffer {
        let mut guard = self.buffers.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(buffer) = guard.pop_front() {
            return PooledBuffer {
                buffer: Some(buffer),
                pool: Arc::clone(self),
            };
        }

        // Pool empty - allocate fresh. No hard limit since buffers recycle on drop.
        let mut total = self.total_created.lock().unwrap_or_else(|e| e.into_inner());
        *total += 1;
        PooledBuffer {
            buffer: Some(vec![0u8; self.buffer_size]),
            pool: Arc::clone(self),
        }
    }

    fn release(&self, buffer: Vec<u8>) {
        let mut guard = self.buffers.lock().unwrap_or_else(|e| e.into_inner());
        guard.push_back(buffer);
    }

    pub(super) fn buffer_size(&self) -> usize {
        self.buffer_size
    }

    pub(super) fn trim_to(&self, target: usize) {
        let mut guard = self.buffers.lock().unwrap_or_else(|e| e.into_inner());
        while guard.len() > target {
            guard.pop_back();
        }
    }
}

/// A buffer that returns to its pool on drop.
pub(super) struct PooledBuffer {
    buffer: Option<Vec<u8>>,
    pool: Arc<FramePool>,
}

impl PooledBuffer {
    /// Take the buffer out, consuming the wrapper without returning to pool.
    /// Use this when the buffer will be owned by another type (e.g., RgbaImage).
    pub(super) fn into_inner(mut self) -> Vec<u8> {
        self.buffer.take().unwrap_or_default()
    }
}

impl std::ops::Deref for PooledBuffer {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        self.buffer.as_deref().unwrap_or(&[])
    }
}

impl std::ops::DerefMut for PooledBuffer {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.buffer.as_deref_mut().unwrap_or(&mut [])
    }
}

impl Drop for PooledBuffer {
    fn drop(&mut self) {
        if let Some(buffer) = self.buffer.take() {
            self.pool.release(buffer);
        }
    }
}

/// Wrapper around RgbaImage that returns the underlying buffer to a pool on drop.
/// This prevents memory leaks in the decode pipeline where many frames are decoded and displayed.
pub struct PooledRgbaImage {
    image: Option<image::RgbaImage>,
    width: u32,
    height: u32,
    pool: Arc<FramePool>,
}

impl PooledRgbaImage {
    /// Create a new pooled image from a PooledBuffer.
    /// The buffer is taken and wrapped into an RgbaImage.
    pub(super) fn from_pooled_buffer(buffer: PooledBuffer, width: u32, height: u32) -> Option<Self> {
        let pool = Arc::clone(&buffer.pool);
        let vec = buffer.into_inner();
        let image = image::RgbaImage::from_raw(width, height, vec)?;
        Some(Self {
            image: Some(image),
            width,
            height,
            pool,
        })
    }

    /// Access the underlying RgbaImage.
    pub(super) fn as_image(&self) -> Option<&image::RgbaImage> {
        self.image.as_ref()
    }
}

impl std::ops::Deref for PooledRgbaImage {
    type Target = image::RgbaImage;

    fn deref(&self) -> &Self::Target {
        self.image.as_ref().expect("PooledRgbaImage already consumed")
    }
}

impl Drop for PooledRgbaImage {
    fn drop(&mut self) {
        if let Some(image) = self.image.take() {
            // Extract the underlying Vec<u8> from the RgbaImage and return it to the pool.
            // RgbaImage stores data as ImageBuffer<Vec<u8>>, so into_raw() gives us the Vec.
            let buffer = image.into_raw();
            // Only return buffers of the expected size (prevents issues if dimensions changed)
            if buffer.len() == self.pool.buffer_size() {
                self.pool.release(buffer);
            }
            // Otherwise the buffer is dropped - this handles edge cases like resize
        }
    }
}
