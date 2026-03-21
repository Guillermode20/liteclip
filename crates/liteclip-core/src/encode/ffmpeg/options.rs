use ffmpeg_next::format::Pixel;

use crate::config::{QualityPreset, RateControl};
use crate::encode::encoder_mod::ResolvedEncoderType;
use crate::encode::EncodeResult;

use super::FfmpegEncoder;

impl FfmpegEncoder {
    pub(super) fn apply_codec_specific_options(
        &self,
        options: &mut ffmpeg_next::Dictionary<'_>,
        bitrate: usize,
    ) -> EncodeResult<()> {
        match self.config.encoder_type {
            ResolvedEncoderType::Nvenc => self.apply_nvenc_options(options, bitrate),
            ResolvedEncoderType::Amf => self.apply_amf_options(options, bitrate),
            ResolvedEncoderType::Qsv => self.apply_qsv_options(options, bitrate),
        }

        Ok(())
    }

    pub(super) fn encoder_pixel_format(&self) -> Pixel {
        match self.config.encoder_type {
            ResolvedEncoderType::Nvenc | ResolvedEncoderType::Amf | ResolvedEncoderType::Qsv => {
                Pixel::NV12
            }
        }
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
