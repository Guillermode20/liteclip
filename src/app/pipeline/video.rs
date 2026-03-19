use crate::{
    buffer::ReplayBuffer,
    capture::{CaptureBackend, CaptureConfig, CaptureFactory, CapturedFrame},
    config::Config,
    encode::{resolve_effective_encoder_config, EncoderConfig, EncoderFactory, EncoderHandle},
};
use anyhow::{bail, Context, Result};
use crossbeam::channel::Receiver;
use tracing::{info, warn};

pub fn start_video_pipeline(
    config: &Config,
    buffer: &ReplayBuffer,
    capture_factory: &dyn CaptureFactory,
    encoder_factory: &dyn EncoderFactory,
) -> Result<(Box<dyn CaptureBackend>, EncoderHandle)> {
    let requested_encoder_config = EncoderConfig::from(config);
    let mut encoder_config = resolve_effective_encoder_config(&requested_encoder_config)?;

    let mut capture = capture_factory
        .create()
        .context("Failed to create capture backend")?;
    let mut capture_config = CaptureConfig::from(config);

    let encoder_supports_gpu = encoder_config.supports_gpu_frame_transport();
    #[cfg(windows)]
    let requested_gpu_format = encoder_config.gpu_texture_format();
    let capture_supports_nv12 =
        capture_factory.refresh_nv12_capability(capture_config.output_index);

    if encoder_supports_gpu {
        #[cfg(windows)]
        if requested_gpu_format == Some(crate::capture::GpuTextureFormat::Nv12)
            && !capture_supports_nv12
        {
            bail!(
                "GPU encoder {:?} requires NV12 conversion capability, but the capture device (output {}) does not support it. Please try a different display output or encoder.",
                encoder_config.encoder_type,
                capture_config.output_index
            );
        }
        info!(
            "GPU transport enabled: encoder={:?}, output={}, format={:?}",
            encoder_config.encoder_type, capture_config.output_index, requested_gpu_format
        );
        capture_config.perform_cpu_readback = false;
        #[cfg(windows)]
        if let Some(gpu_texture_format) = requested_gpu_format {
            capture_config.gpu_texture_format = gpu_texture_format;
        }
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
    let encoder_handle = encoder_factory
        .spawn(encoder_config, buffer.clone(), frame_rx)
        .context("Failed to spawn encoder")?;

    Ok((capture, encoder_handle))
}
