//! The GitHub release feed the app installs and updates from.
//!
//! Each release publishes a `release.json` (version + per-asset SHA256s) plus
//! the assets themselves. We resolve the latest release through the GitHub API
//! so we get real `browser_download_url`s, then read `release.json` for the
//! versions and checksums to verify against.

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::download;

/// The upstream repository that publishes mite releases.
pub const REPO: &str = "jssblck/mite";

/// `release.json` as published by the release workflow.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseManifest {
    pub version: String,
    pub cli: AssetRef,
    #[serde(default)]
    pub gpu_runtime: Option<AssetRef>,
    pub model_manifest: AssetName,
    // The app-shell installer asset. Reserved for signed app self-update, which
    // is the documented follow-up (it needs signing keys); not consumed yet.
    #[serde(default)]
    #[allow(dead_code)]
    pub installer: Option<AssetRef>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AssetRef {
    pub asset: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AssetName {
    pub asset: String,
}

#[derive(Debug, Deserialize)]
struct GhRelease {
    tag_name: String,
    assets: Vec<GhAsset>,
}

#[derive(Debug, Clone, Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
}

/// The latest release: its tag, its parsed `release.json`, and the asset map
/// used to resolve download URLs by filename.
pub struct LatestRelease {
    pub tag: String,
    pub manifest: ReleaseManifest,
    assets: Vec<GhAsset>,
}

impl LatestRelease {
    /// The `browser_download_url` for an asset filename, if present.
    pub fn asset_url(&self, name: &str) -> Option<String> {
        self.assets
            .iter()
            .find(|asset| asset.name == name)
            .map(|asset| asset.browser_download_url.clone())
    }
}

/// Resolve the latest published release and its manifest.
pub fn fetch_latest() -> Result<LatestRelease> {
    let api = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let release: GhRelease =
        download::get_json(&api).with_context(|| format!("querying latest release for {REPO}"))?;

    let manifest_url = release
        .assets
        .iter()
        .find(|asset| asset.name == "release.json")
        .map(|asset| asset.browser_download_url.clone())
        .context("latest release is missing release.json")?;

    let manifest: ReleaseManifest =
        download::get_json(&manifest_url).context("parsing release.json")?;

    Ok(LatestRelease {
        tag: release.tag_name,
        manifest,
        assets: release.assets,
    })
}

/// Strip a leading `v` so `v0.1.0` and `0.1.0` compare equal.
pub fn normalize_version(version: &str) -> &str {
    version.strip_prefix('v').unwrap_or(version)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_release_manifest() {
        let json = r#"{
            "version": "v0.1.0",
            "cli": { "asset": "mite.exe", "sha256": "abc" },
            "gpuRuntime": { "asset": "mite-gpu-runtime-win64.zip", "sha256": "def" },
            "modelManifest": { "asset": "model-manifest.json" },
            "installer": { "asset": "mite-setup.exe", "sha256": "ghi" }
        }"#;
        let manifest: ReleaseManifest = serde_json::from_str(json).unwrap();
        assert_eq!(manifest.version, "v0.1.0");
        assert_eq!(manifest.cli.asset, "mite.exe");
        assert_eq!(manifest.gpu_runtime.unwrap().sha256, "def");
        assert_eq!(manifest.model_manifest.asset, "model-manifest.json");
    }

    #[test]
    fn normalizes_versions() {
        assert_eq!(normalize_version("v0.1.0"), "0.1.0");
        assert_eq!(normalize_version("0.1.0"), "0.1.0");
    }
}
