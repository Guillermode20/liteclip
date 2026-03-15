use anyhow::{bail, Result};
use ffmpeg_next::format::Pixel;

use crate::config::{EncoderType, QualityPreset, RateControl};

use super::FfmpegEncoder;

impl FfmpegEncoder {
    pub(super) fn apply_codec_specific_options(
        &self,
        options: &mut ffmpeg_next::Dictionary<'_>,
        bitrate: usize,
    ) -> Result<()> {
        match self.config.encoder_type {
            EncoderType::Nvenc => self.apply_nvenc_options(options, bitrate),
            EncoderType::Amf => self.apply_amf_options(options, bitrate),
            EncoderType::Qsv => self.apply_qsv_options(options, bitrate),
            EncoderType::Auto => bail!("auto encoder type should be resolved before init"),
        }

        Ok(())
    }

    pub(super) fn encoder_pixel_format(&self) -> Pixel {
        match self.config.encoder_type {
            EncoderType::Nvenc | EncoderType::Amf | EncoderType::Qsv | EncoderType::Auto => {
                Pixel::NV12
            }
        }
    }

    #[allow(dead_code)]
    pub(super) fn hardware_frame_sw_format(&self) -> Pixel {
        self.encoder_pixel_format()
    }

    pub(super) fn bitrate_bps(&self) -> usize {
        (self.config.bitrate_mbps.max(1) * 1_000_000) as usize
    }

    pub(super) fn peak_bitrate_bps(&self) -> usize {
        match self.config.rate_control {
            RateControl::Cbr => self.bitrate_bps(),
            RateControl::Vbr | RateControl::Cq => self.bitrate_bps().saturating_mul(2),
        }
    }

    pub(super) fn cq_value(&self) -> u8 {
        self.config
            .quality_value
            .unwrap_or(match self.config.quality_preset {
                QualityPreset::Performance => 28,
                QualityPreset::Balanced => 23,
                QualityPreset::Quality => 19,
            })
    }

    pub(super) fn nvenc_preset(&self) -> &'static str {
        match self.config.quality_preset {
            QualityPreset::Performance => "p3",
            QualityPreset::Balanced => "p5",
            QualityPreset::Quality => "p7",
        }
    }

    pub(super) fn nvenc_tune(&self) -> &'static str {
        match self.config.quality_preset {
            QualityPreset::Performance => "ull",
            QualityPreset::Balanced => "ll",
            QualityPreset::Quality => "hq",
        }
    }

    pub(super) fn qsv_preset(&self) -> &'static str {
        match self.config.quality_preset {
            QualityPreset::Performance => "veryfast",
            QualityPreset::Balanced => "faster",
            QualityPreset::Quality => "medium",
        }
    }

    pub(super) fn amf_quality(&self) -> &'static str {
        match self.config.quality_preset {
            QualityPreset::Performance => "speed",
            QualityPreset::Balanced => "balanced",
            QualityPreset::Quality => "quality",
        }
    }

    pub(super) fn amf_rc_mode(&self) -> &'static str {
        match self.config.rate_control {
            RateControl::Cbr => "cbr",
            RateControl::Vbr | RateControl::Cq => "vbr_latency",
        }
    }
}
