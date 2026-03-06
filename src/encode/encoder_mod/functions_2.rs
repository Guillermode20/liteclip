//! Auto-generated module
//!
//! 🤖 Generated with [SplitRS](https://github.com/cool-japan/splitrs)

#[cfg(test)]
mod tests {
    use super::super::functions::apply_auto_encoder_selection;
    use super::super::types::{EncodedPacket, EncoderConfig, HardwareEncoder, StreamType};
    #[test]
    fn test_encoder_config_codec_names() {
        let mut config = EncoderConfig::new(
            crate::config::Codec::H264,
            20,
            30,
            (1920, 1080),
            crate::config::EncoderType::Nvenc,
            1,
        );
        assert_eq!(config.ffmpeg_codec_name(), "h264_nvenc");
        config.encoder_type = crate::config::EncoderType::Amf;
        assert_eq!(config.ffmpeg_codec_name(), "h264_amf");
        config.encoder_type = crate::config::EncoderType::Qsv;
        assert_eq!(config.ffmpeg_codec_name(), "h264_qsv");
        config.encoder_type = crate::config::EncoderType::Software;
        assert_eq!(config.ffmpeg_codec_name(), "libx264");
    }
    #[test]
    fn test_keyframe_interval_calculation() {
        let config = EncoderConfig::new(
            crate::config::Codec::H264,
            20,
            30,
            (1920, 1080),
            crate::config::EncoderType::Nvenc,
            2,
        );
        assert_eq!(config.keyframe_interval_frames(), 60);
    }
    #[test]
    fn test_encoder_config_new_sets_quality_defaults() {
        let config = EncoderConfig::new(
            crate::config::Codec::H264,
            20,
            30,
            (1920, 1080),
            crate::config::EncoderType::Nvenc,
            1,
        );
        assert_eq!(
            config.quality_preset,
            crate::config::QualityPreset::Balanced
        );
        assert_eq!(config.rate_control, crate::config::RateControl::Vbr);
        assert_eq!(config.quality_value, None);
    }
    #[test]
    fn test_encoded_packet_creation() {
        let packet = EncodedPacket::video_keyframe(vec![0u8; 1024], 1_000_000);
        assert_eq!(packet.data.len(), 1024);
        assert_eq!(packet.pts, 1_000_000);
        assert!(packet.is_keyframe);
        assert!(matches!(packet.stream, StreamType::Video));
        let packet = EncodedPacket::video_delta(vec![0u8; 512], 2_000_000);
        assert!(!packet.is_keyframe);
    }
    #[test]
    fn test_hardware_encoder_conversion() {
        assert!(matches!(
            HardwareEncoder::Nvenc.into(),
            crate::config::EncoderType::Nvenc
        ));
        assert!(matches!(
            HardwareEncoder::Amf.into(),
            crate::config::EncoderType::Amf
        ));
        assert!(matches!(
            HardwareEncoder::Qsv.into(),
            crate::config::EncoderType::Qsv
        ));
        assert!(matches!(
            HardwareEncoder::None.into(),
            crate::config::EncoderType::Software
        ));
    }

    #[test]
    fn test_auto_encoder_selection_prefers_software_h264_when_no_hw_hevc() {
        let mut config = EncoderConfig::new(
            crate::config::Codec::H265,
            20,
            60,
            (1920, 1080),
            crate::config::EncoderType::Auto,
            2,
        );

        apply_auto_encoder_selection(&mut config, HardwareEncoder::None);

        assert_eq!(config.encoder_type, crate::config::EncoderType::Software);
        assert_eq!(config.codec, crate::config::Codec::H264);
    }

    #[test]
    fn test_auto_encoder_selection_preserves_codec_when_hw_available() {
        let mut config = EncoderConfig::new(
            crate::config::Codec::H265,
            20,
            30,
            (1920, 1080),
            crate::config::EncoderType::Auto,
            2,
        );

        apply_auto_encoder_selection(&mut config, HardwareEncoder::Amf);

        assert_eq!(config.encoder_type, crate::config::EncoderType::Amf);
        assert_eq!(config.codec, crate::config::Codec::H265);
    }

    #[test]
    fn test_auto_encoder_selection_prefers_h264_for_amf_realtime_hevc() {
        let mut config = EncoderConfig::new(
            crate::config::Codec::H265,
            10,
            60,
            (1920, 1080),
            crate::config::EncoderType::Auto,
            1,
        );

        apply_auto_encoder_selection(&mut config, HardwareEncoder::Amf);

        assert_eq!(config.encoder_type, crate::config::EncoderType::Amf);
        assert_eq!(config.codec, crate::config::Codec::H264);
        assert_eq!(config.keyframe_interval_secs, 2);
        assert_eq!(
            config.quality_preset,
            crate::config::QualityPreset::Performance
        );
    }
}
