//! Shared video frame types for capture and encode.
//!
//! Lives outside the `capture` module so the encoder stack does not depend on
//! capture-backend traits or DXGI plumbing.

use bytes::Bytes;
use std::sync::Arc;
#[cfg(windows)]
use windows::Win32::Foundation::HANDLE;
#[cfg(windows)]
use windows::Win32::Graphics::Direct3D11::{
    ID3D11Device, ID3D11Texture2D, ID3D11VideoProcessorOutputView,
};

/// GPU texture format for D3D11 frames.
#[cfg(windows)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuTextureFormat {
    /// BGRA format (B8G8R8A8_UNORM).
    Bgra,
    /// NV12 format (YUV 4:2:0).
    Nv12,
}

/// Pool item for recycled D3D11 textures.
#[cfg(windows)]
pub struct D3d11TexturePoolItem {
    pub texture: ID3D11Texture2D,
    pub output_view: Option<ID3D11VideoProcessorOutputView>,
    pub shared_handle: HANDLE,
}

#[cfg(windows)]
impl Drop for D3d11TexturePoolItem {
    fn drop(&mut self) {
        // We intentionality do NOT call CloseHandle on self.shared_handle.
        // Since textures are created with D3D11_RESOURCE_MISC_SHARED (not D3D11_RESOURCE_MISC_SHARED_NTHANDLE),
        // the handle is a legacy KMT handle. Resolving or closing it explicitly via `CloseHandle` is invalid
        // and can corrupt the NT handle table or throw STATUS_INVALID_HANDLE.
        // The D3D11 driver internally tracks KMT handle lifetimes via the original texture's COM reference count.
    }
}

#[cfg(windows)]
pub struct D3d11Frame {
    pub device: ID3D11Device,
    pub format: GpuTextureFormat,
    pub fence_value: u64,
    pub fence_shared_handle: Option<HANDLE>,
    pub pool_item: Option<D3d11TexturePoolItem>,
    pub return_tx: Option<crossbeam::channel::Sender<D3d11TexturePoolItem>>,
}

#[cfg(windows)]
impl Drop for D3d11Frame {
    fn drop(&mut self) {
        if let (Some(item), Some(tx)) = (self.pool_item.take(), self.return_tx.take()) {
            let _ = tx.send(item);
        }
    }
}

// SAFETY: D3d11Frame is Send because:
// 1. ID3D11Texture2D and ID3D11Device are COM interfaces that are thread-safe
// 2. HANDLE is a simple copyable value (pointer-sized integer)
// 3. GpuTextureFormat is a simple enum (Copy)
// 4. The fence values are plain u64 values
// 5. Channels and Options containing them are Send.
// The frame is passed between capture and encoder threads via Arc
#[cfg(windows)]
unsafe impl Send for D3d11Frame {}

// SAFETY: D3d11Frame is Sync because:
// 1. The frame is wrapped in Arc<D3d11Frame> when shared between threads
// 2. All fields are either immutable after creation or use interior mutability
//    via the COM interfaces which have their own thread safety guarantees
// 3. The recycle mechanism uses channel communication which is thread-safe
#[cfg(windows)]
unsafe impl Sync for D3d11Frame {}

#[cfg(windows)]
impl D3d11Frame {
    pub(crate) fn from_pooled(
        device: ID3D11Device,
        format: GpuTextureFormat,
        pool_item: D3d11TexturePoolItem,
        fence_value: u64,
        fence_shared_handle: Option<HANDLE>,
        return_tx: crossbeam::channel::Sender<D3d11TexturePoolItem>,
    ) -> Self {
        Self {
            device,
            format,
            fence_value,
            fence_shared_handle,
            pool_item: Some(pool_item),
            return_tx: Some(return_tx),
        }
    }

    pub fn texture(&self) -> Option<&ID3D11Texture2D> {
        self.pool_item.as_ref().map(|i| &i.texture)
    }

    pub fn shared_handle(&self) -> Option<HANDLE> {
        self.pool_item.as_ref().map(|i| i.shared_handle)
    }
}

/// Captured frame data for the encoding pipeline (CPU and/or GPU paths).
#[derive(Clone)]
pub struct CapturedFrame {
    pub bgra: Bytes,
    #[cfg(windows)]
    pub d3d11: Option<Arc<D3d11Frame>>,
    pub timestamp: i64,
    pub resolution: (u32, u32),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_frame() -> CapturedFrame {
        CapturedFrame {
            bgra: Bytes::from(vec![0u8; 1920 * 1080 * 4]),
            #[cfg(windows)]
            d3d11: None,
            timestamp: 1000,
            resolution: (1920, 1080),
        }
    }

    #[test]
    fn captured_frame_resolution() {
        let frame = make_test_frame();
        assert_eq!(frame.resolution, (1920, 1080));
    }

    #[test]
    fn captured_frame_timestamp() {
        let frame = make_test_frame();
        assert_eq!(frame.timestamp, 1000);
    }

    #[test]
    fn bgra_cheap_clone() {
        let frame = make_test_frame();
        let original_ptr = frame.bgra.as_ptr();
        let cloned = frame.clone();
        let cloned_ptr = cloned.bgra.as_ptr();
        assert_eq!(
            original_ptr, cloned_ptr,
            "Bytes clone should share the same underlying pointer (ref count bump)"
        );
    }

    #[test]
    fn captured_frame_clone_preserves_fields() {
        let frame = make_test_frame();
        let cloned = frame.clone();
        assert_eq!(cloned.resolution, frame.resolution);
        assert_eq!(cloned.timestamp, frame.timestamp);
        assert_eq!(cloned.bgra.len(), frame.bgra.len());
    }
}
