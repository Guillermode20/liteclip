use crate::{
    buffer::ReplayBuffer,
    capture::{CaptureBackend, CaptureConfig, CapturedFrame, DxgiCapture},
    config::Config,
    encode::{
        resolve_effective_encoder_config, spawn_encoder_with_receiver, EncoderConfig, EncoderHandle,
    },
};
use anyhow::{bail, Context, Result};
use crossbeam::channel::Receiver;
use tracing::{info, warn};

pub fn start_video_pipeline(
    config: &Config,
    buffer: &ReplayBuffer,
) -> Result<(DxgiCapture, EncoderHandle)> {
    let requested_encoder_config = EncoderConfig::from(config);
    let mut encoder_config = resolve_effective_encoder_config(&requested_encoder_config);

    let mut capture = DxgiCapture::new().context("Failed to create DXGI capture")?;
    let mut capture_config = CaptureConfig::from(config);

    let encoder_supports_gpu = encoder_config.supports_gpu_frame_transport();
    let capture_supports_nv12 =
        capture.refresh_nv12_conversion_capability(capture_config.output_index);

    if encoder_supports_gpu {
        if !capture_supports_nv12 {
            bail!(
                "GPU encoder {:?} requires NV12 conversion capability, but the capture device (output {}) does not support it. Please try a different display output or encoder.",
                encoder_config.encoder_type,
                capture_config.output_index
            );
        }
        info!(
            "GPU NV12 transport enabled: encoder={:?}, output={}",
            encoder_config.encoder_type, capture_config.output_index
        );
        capture_config.perform_cpu_readback = false;
        encoder_config.use_cpu_readback = false;
    } else {
        warn!(
            "GPU transport not available: encoder {:?} does not support GPU frame transport, using CPU readback",
            encoder_config.encoder_type
        );
        capture_config.perform_cpu_readback = true;
        encoder_config.use_cpu_readback = true;
    }

    capture
        .start(capture_config)
        .context("Failed to start capture")?;

    let frame_rx: Receiver<CapturedFrame> = capture.frame_rx();
    let encoder_handle = spawn_encoder_with_receiver(encoder_config, buffer.clone(), frame_rx)
        .context("Failed to spawn encoder")?;

    Ok((capture, encoder_handle))
}
