//! Shared video frame types for capture and encode.
//!
//! Lives outside the `capture` module so the encoder stack does not depend on
//! capture-backend traits or DXGI plumbing.

use bytes::Bytes;
#[cfg(windows)]
use crossbeam::channel::Sender;
use std::sync::Arc;
#[cfg(windows)]
use windows::Win32::Foundation::{CloseHandle, HANDLE};
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
        // Close the DXGI shared handle obtained from IDXGIResource::GetSharedHandle.
        // The underlying ID3D11Texture2D COM object is released by its own Drop impl,
        // but the shared handle is a separate kernel object that requires CloseHandle.
        if !self.shared_handle.is_invalid() {
            unsafe {
                let _ = CloseHandle(self.shared_handle);
            }
        }
    }
}

#[cfg(windows)]
struct D3d11TextureRecycle {
    return_tx: Sender<D3d11TexturePoolItem>,
    item: Option<D3d11TexturePoolItem>,
}

#[cfg(windows)]
impl Drop for D3d11TextureRecycle {
    fn drop(&mut self) {
        if let Some(item) = self.item.take() {
            let _ = self.return_tx.try_send(item);
        }
    }
}

#[cfg(windows)]
pub struct D3d11Frame {
    pub texture: ID3D11Texture2D,
    pub device: ID3D11Device,
    pub format: GpuTextureFormat,
    pub shared_handle: HANDLE,
    pub fence_value: u64,
    pub fence_shared_handle: Option<HANDLE>,
    #[allow(dead_code)]
    recycle: Option<D3d11TextureRecycle>,
}

#[cfg(windows)]
unsafe impl Send for D3d11Frame {}

#[cfg(windows)]
unsafe impl Sync for D3d11Frame {}

#[cfg(windows)]
impl D3d11Frame {
    pub(crate) fn from_pooled(
        device: ID3D11Device,
        format: GpuTextureFormat,
        return_tx: Sender<D3d11TexturePoolItem>,
        pool_item: D3d11TexturePoolItem,
        fence_value: u64,
        fence_shared_handle: Option<HANDLE>,
    ) -> Self {
        let texture = pool_item.texture.clone();
        let shared_handle = pool_item.shared_handle;
        Self {
            texture,
            device,
            format,
            shared_handle,
            fence_value,
            fence_shared_handle,
            recycle: Some(D3d11TextureRecycle {
                return_tx,
                item: Some(pool_item),
            }),
        }
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
