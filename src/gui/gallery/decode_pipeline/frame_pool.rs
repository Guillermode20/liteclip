use std::collections::VecDeque;
use std::sync::Mutex;

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

    pub(super) fn acquire(&self) -> Option<Vec<u8>> {
        let mut guard = self.buffers.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(buffer) = guard.pop_front() {
            return Some(buffer);
        }

        let mut total = self.total_created.lock().unwrap_or_else(|e| e.into_inner());
        if *total >= FRAME_POOL_HARD_LIMIT {
            return None;
        }
        *total += 1;
        Some(vec![0u8; self.buffer_size])
    }

    #[allow(dead_code)]
    pub(super) fn release(&self, buffer: Vec<u8>) {
        let mut guard = self.buffers.lock().unwrap_or_else(|e| e.into_inner());
        if guard.len() < FRAME_POOL_HARD_LIMIT {
            guard.push_back(buffer);
        }
    }

    pub(super) fn buffer_size(&self) -> usize {
        self.buffer_size
    }
}
