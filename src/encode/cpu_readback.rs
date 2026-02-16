//! CPU Readback Path
//!
//! Fallback path that copies GPU textures to CPU memory for software encoding.
//!
//! This is a stub implementation for Phase 1.

use crate::d3d::{D3D11Device, D3D11Texture};
use anyhow::Result;
use std::sync::Arc;

/// CPU readback buffer
pub struct CpuReadbackBuffer {
    width: u32,
    height: u32,
    // Pre-allocated buffer to avoid repeated allocations
    buffer: Vec<u8>,
}

impl CpuReadbackBuffer {
    /// Create a new CPU readback buffer
    pub fn new(width: u32, height: u32) -> Result<Self> {
        let size = (width * height * 4) as usize;
        let buffer = vec![0u8; size];
        Ok(Self {
            width,
            height,
            buffer,
        })
    }

    /// Copy texture to CPU memory
    pub fn copy_texture(&mut self, _texture: &D3D11Texture) -> Result<Vec<u8>> {
        // Stub: return empty buffer
        // In a real implementation, this would copy the texture data
        // For now, we return a reference to the pre-allocated buffer
        Ok(self.buffer.clone())
    }

    /// Get dimensions
    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }
}

/// Readback stage for copying GPU textures to CPU
pub struct CpuReadbackStage {
    #[allow(dead_code)]
    device: Arc<D3D11Device>,
    width: u32,
    height: u32,
    // Pre-allocated buffer to avoid repeated allocations
    buffer: Vec<u8>,
}

impl CpuReadbackStage {
    /// Create a new CPU readback stage
    pub fn new(device: &Arc<D3D11Device>, width: u32, height: u32) -> Result<Self> {
        let size = (width * height * 4) as usize;
        let buffer = vec![0u8; size];
        Ok(Self {
            device: Arc::clone(device),
            width,
            height,
            buffer,
        })
    }

    /// Copy texture to CPU memory
    pub fn readback(&self, _texture: &D3D11Texture) -> Result<Vec<u8>> {
        // Stub implementation
        // In a real implementation, this would copy the texture data
        // For now, we return a reference to the pre-allocated buffer
        Ok(self.buffer.clone())
    }

    /// Get dimensions
    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cpu_readback_buffer() {
        let buffer = CpuReadbackBuffer::new(1920, 1080).unwrap();
        assert_eq!(buffer.dimensions(), (1920, 1080));
    }
}
