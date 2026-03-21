use crate::buffer::ReplayBuffer;
use crate::capture::WebcamCapture;
use crate::config::Config;
use crate::encode::{
    resolve_effective_encoder_config, EncoderConfig, EncoderFactory, EncoderHandle,
};
use anyhow::{Context, Result};
use crossbeam::channel::bounded;

/// Encoder settings for the webcam branch: fixed size, CPU BGRA path.
pub fn webcam_encoder_config(config: &Config) -> EncoderConfig {
    let mut e = EncoderConfig::from(config);
    e.resolution = (config.video.webcam_width, config.video.webcam_height);
    e.use_native_resolution = false;
    e.use_cpu_readback = true;
    e
}

/// Starts dshow capture and a second encoder writing into `webcam_buffer`.
pub fn start_webcam_pipeline(
    config: &Config,
    webcam_buffer: &ReplayBuffer,
    encoder_factory: &dyn EncoderFactory,
    webcam_capture: &mut WebcamCapture,
) -> Result<EncoderHandle> {
    let enc_cfg = webcam_encoder_config(config);
    let resolved = resolve_effective_encoder_config(&enc_cfg).context("webcam encoder config")?;
    let (frame_tx, frame_rx) = bounded(32);
    webcam_capture
        .start_webcam_with_options(
            &config.video.webcam_device_name,
            config.video.webcam_width,
            config.video.webcam_height,
            config.video.framerate,
            frame_tx,
        )
        .context("webcam capture start")?;
    encoder_factory
        .spawn(resolved, webcam_buffer.clone(), frame_rx)
        .map_err(|e| anyhow::anyhow!("webcam encoder: {}", e))
}
