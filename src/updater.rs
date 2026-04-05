//! GitHub Releases update checker.
//!
//! Queries the GitHub API for the latest release and compares the tag version
//! against the compiled-in `CARGO_PKG_VERSION`. Designed for manual invocation
//! from the Settings GUI.

use anyhow::{Context, Result};
use serde::Deserialize;
use tracing::{debug, info};

const REPO_OWNER: &str = "Guillermode20";
const REPO_NAME: &str = "liteclip-recorder";
const API_URL: &str =
    "https://api.github.com/repos/Guillermode20/liteclip-recorder/releases/latest";

/// Information about an available update.
#[derive(Debug, Clone)]
pub struct UpdateInfo {
    /// The version tag (e.g. `"0.3.0"`).
    pub version: String,
    /// URL to the GitHub release page.
    pub release_url: String,
}

/// Parsed subset of the GitHub release API response.
#[derive(Debug, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    html_url: String,
    prerelease: bool,
}

/// Check GitHub Releases for a newer version.
///
/// Returns `Ok(Some(info))` if a newer release is found, `Ok(None)` if
/// the current version is up to date.
pub fn check_for_updates() -> Result<Option<UpdateInfo>> {
    let current = env!("CARGO_PKG_VERSION");
    info!("Checking for updates (current: {})", current);

    let release: GitHubRelease = ureq::Agent::config_builder()
        .user_agent(format!(
            "LiteClip/{} ({}/{})",
            current, REPO_OWNER, REPO_NAME
        ))
        .build()
        .new_agent()
        .get(API_URL)
        .call()
        .context("Failed to reach GitHub API")?
        .body_mut()
        .read_json()
        .context("Failed to parse GitHub release JSON")?;

    let latest_tag = release.tag_name.trim().trim_start_matches('v');
    debug!(
        "Latest release tag: {} (prerelease: {})",
        latest_tag, release.prerelease
    );

    // Skip pre-releases unless the current version is also a pre-release
    if release.prerelease && !is_prerelease(current) {
        debug!("Skipping pre-release {}", latest_tag);
        return Ok(None);
    }

    if is_newer(latest_tag, current)? {
        info!("Update available: {} -> {}", current, latest_tag);
        Ok(Some(UpdateInfo {
            version: latest_tag.to_string(),
            release_url: release.html_url,
        }))
    } else {
        info!("Already up to date ({})", current);
        Ok(None)
    }
}

/// Detect whether the app is installed via MSI (Program Files) or is portable.
///
/// Returns `"MSI"` or `"Portable"`.
pub fn install_type() -> &'static str {
    match std::env::current_exe() {
        Ok(exe) => {
            let path = exe.to_string_lossy().to_lowercase();
            if path.contains("program files") || path.contains("programfiles") {
                "MSI"
            } else {
                "Portable"
            }
        }
        Err(_) => "Portable",
    }
}

/// Compare two semver-like version strings.
///
/// Returns `true` if `latest > current`.
fn is_newer(latest: &str, current: &str) -> Result<bool> {
    let l = parse_version(latest).context("Failed to parse latest version")?;
    let c = parse_version(current).context("Failed to parse current version")?;
    Ok(l > c)
}

fn parse_version(v: &str) -> Result<[u64; 3]> {
    // Strip any pre-release suffix for comparison
    let base = v.split('-').next().unwrap_or(v);
    let parts: Vec<u64> = base.split('.').filter_map(|s| s.parse().ok()).collect();
    anyhow::ensure!(parts.len() >= 2, "version must have at least major.minor");
    Ok([
        *parts.first().unwrap_or(&0),
        *parts.get(1).unwrap_or(&0),
        *parts.get(2).unwrap_or(&0),
    ])
}

fn is_prerelease(v: &str) -> bool {
    v.contains("-alpha") || v.contains("-beta") || v.contains("-rc")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_version_major_minor_patch() {
        assert_eq!(parse_version("1.2.3").unwrap(), [1, 2, 3]);
    }

    #[test]
    fn parse_version_major_minor() {
        assert_eq!(parse_version("1.2").unwrap(), [1, 2, 0]);
    }

    #[test]
    fn parse_version_with_prerelease() {
        assert_eq!(parse_version("1.2.3-beta.1").unwrap(), [1, 2, 3]);
    }

    #[test]
    fn newer_patch() {
        assert!(is_newer("0.2.1", "0.2.0").unwrap());
        assert!(!is_newer("0.2.0", "0.2.1").unwrap());
    }

    #[test]
    fn newer_minor() {
        assert!(is_newer("0.3.0", "0.2.9").unwrap());
        assert!(!is_newer("0.2.9", "0.3.0").unwrap());
    }

    #[test]
    fn newer_major() {
        assert!(is_newer("1.0.0", "0.9.9").unwrap());
    }

    #[test]
    fn equal_versions() {
        assert!(!is_newer("0.2.0", "0.2.0").unwrap());
    }

    #[test]
    fn prerelease_detection() {
        assert!(is_prerelease("0.2.0-alpha"));
        assert!(is_prerelease("0.2.0-beta.1"));
        assert!(is_prerelease("0.2.0-rc.2"));
        assert!(!is_prerelease("0.2.0"));
    }
}
