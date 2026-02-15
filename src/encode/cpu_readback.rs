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
}

impl CpuReadbackBuffer {
    /// Create a new CPU readback buffer
    pub fn new(width: u32, height: u32) -> Result<Self> {
        Ok(Self { width, height })
    }

    /// Copy texture to CPU memory
    pub fn copy_texture(&mut self, _texture: &D3D11Texture) -> Result<Vec<u8>> {
        // Stub: return empty buffer
        let size = (self.width * self.height * 4) as usize;
        Ok(vec![0u8; size])
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
}

impl CpuReadbackStage {
    /// Create a new CPU readback stage
    pub fn new(device: &Arc<D3D11Device>, width: u32, height: u32) -> Result<Self> {
        Ok(Self {
            device: Arc::clone(device),
            width,
            height,
        })
    }

    /// Copy texture to CPU memory
    pub fn readback(&self, _texture: &D3D11Texture) -> Result<Vec<u8>> {
        // Stub implementation
        let size = (self.width * self.height * 4) as usize;
        Ok(vec![0u8; size])
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
