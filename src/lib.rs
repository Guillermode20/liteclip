//! LiteClip — desktop application library facade.
//!
//! Re-exports the recording engine from [`liteclip_core`] so existing paths such as
//! `liteclip::app::AppState` remain stable, and adds shell modules for the
//! full product (tray, hotkeys, settings, gallery, game detection).
//!
//! For embedding only the engine in another binary, depend on **`liteclip-core`** directly.

pub use liteclip_core::{
    app, buffer, capture, config, encode, ffmpeg_backend, host, hotkey_parse, media, output, paths,
    quality_contracts, runtime_budget, ReplayEngine,
};

pub mod detection;
pub mod error_log;
pub mod gui;
pub mod platform;

#[cfg(test)]
mod tests {
    #[test]
    fn reexports_are_accessible() {
        let _: fn() -> crate::config::Config = crate::config::Config::default;
        let _ = crate::ReplayEngine::builder;
        let _ = crate::buffer::SharedReplayBuffer::new;
        let _ = crate::media::CapturedFrame {
            bgra: bytes::Bytes::new(),
            #[cfg(windows)]
            d3d11: None,
            timestamp: 0,
            resolution: (0, 0),
        };
    }

    #[test]
    fn config_types_roundtrip_through_reexports() {
        use crate::config::{Config, EncoderType, QualityPreset, Resolution};

        let mut config = Config::default();
        config.video.encoder = EncoderType::Software;
        config.video.quality_preset = QualityPreset::Balanced;
        config.video.resolution = Resolution::P720;

        let serialized = toml::to_string(&config).unwrap();
        let deserialized: Config = toml::from_str(&serialized).unwrap();

        assert_eq!(deserialized.video.encoder, EncoderType::Software);
        assert_eq!(deserialized.video.quality_preset, QualityPreset::Balanced);
        assert_eq!(deserialized.video.resolution, Resolution::P720);
    }

    #[test]
    fn paths_module_accessible() {
        use crate::paths::AppDirs;
        use tempfile::TempDir;

        let temp = TempDir::new().unwrap();
        let config_file = temp.path().join("config.toml");
        let dirs = AppDirs::with_config_file(config_file, "test-binary").unwrap();
        assert_eq!(dirs.clips_folder_name, "test-binary");
    }

    #[test]
    fn host_trait_accessible() {
        use crate::host::CoreHost;
        use std::path::Path;

        struct TestHost;
        impl CoreHost for TestHost {}

        let host = TestHost;
        host.on_clip_saved(Path::new("/test.mp4"));
        host.on_pipeline_fatal("reason");
    }
}
