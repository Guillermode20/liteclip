//! Application directory layout for config and default clip storage.
//!
//! Use [`AppDirs::liteclip_replay`] for the same paths as the LiteClip Replay desktop app.
//! Use [`AppDirs::from_app_slug`] when embedding the engine under your own product id.

use anyhow::{bail, Context, Result};
use std::path::PathBuf;

/// Slug used by LiteClip Replay for `%APPDATA%` and default `Videos` subfolder.
pub const LITECLIP_REPLAY_SLUG: &str = "liteclip-replay";

/// Resolved paths for configuration file and default clip folder naming.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppDirs {
    /// Directory containing the config file (e.g. `%APPDATA%\<slug>`).
    pub config_dir: PathBuf,
    /// Absolute path to the TOML configuration file.
    pub config_file: PathBuf,
    /// Folder name under the user’s `Videos` directory for [`AppDirs::default_save_directory_string`].
    pub clips_folder_name: String,
}

impl AppDirs {
    /// Arbitrary config file location (e.g. temp dir in tests / examples).
    ///
    /// `config_file` must have a parent directory (used as `config_dir`).
    pub fn with_config_file(
        config_file: PathBuf,
        clips_folder_name: impl Into<String>,
    ) -> Result<Self> {
        let clips_folder_name = clips_folder_name.into();
        validate_slug(&clips_folder_name)?;
        let config_dir = config_file
            .parent()
            .context("config_file must have a parent directory")?
            .to_path_buf();
        Ok(Self {
            config_dir,
            config_file,
            clips_folder_name,
        })
    }

    /// Same layout as the LiteClip Replay application (backward compatible).
    ///
    /// Config: `%APPDATA%\liteclip-replay\liteclip-replay.toml`
    pub fn liteclip_replay() -> Result<Self> {
        Self::from_app_slug(LITECLIP_REPLAY_SLUG)
    }

    /// Per-app layout: `%APPDATA%\<slug>\<slug>.toml` and default clips under `Videos\<slug>`.
    ///
    /// `slug` must be 1–64 characters: ASCII letters, digits, `-`, `_` only.
    pub fn from_app_slug(slug: &str) -> Result<Self> {
        validate_slug(slug)?;
        let app_data = dirs::data_dir().context("Failed to get data directory")?;
        let config_dir = app_data.join(slug);
        let config_file = config_dir.join(format!("{slug}.toml"));
        Ok(Self {
            config_dir,
            config_file,
            clips_folder_name: slug.to_string(),
        })
    }

    /// Default `general.save_directory` string matching historic [`crate::config`] defaults, but using [`Self::clips_folder_name`].
    pub fn default_save_directory_string(&self) -> String {
        dirs::video_dir()
            .map(|p| {
                p.join(&self.clips_folder_name)
                    .to_string_lossy()
                    .to_string()
            })
            .unwrap_or_else(|| {
                if let Ok(profile) = std::env::var("USERPROFILE") {
                    format!(r"{profile}\Videos\{}", self.clips_folder_name)
                } else {
                    format!(r"C:\Videos\{}", self.clips_folder_name)
                }
            })
    }
}

fn validate_slug(slug: &str) -> Result<()> {
    if slug.is_empty() || slug.len() > 64 {
        bail!("app slug must be 1–64 characters");
    }
    if !slug
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        bail!("app slug must use only ASCII letters, digits, hyphens, underscores");
    }
    Ok(())
}
