//! The GitHub release feed the app installs and updates from.
//!
//! Each release publishes a `release.json` (version + per-asset SHA256s) plus
//! the assets themselves. The desktop app and the mite CLI ship together in one
//! release, but they update on two separate clocks: the app shell updates itself
//! through `tauri-plugin-updater` (signed, prompted), and it pulls the CLI
//! "engine" to match the app build it is running.
//!
//! "Match" is a caret/semver window, not always-latest: an app build pulls the
//! newest engine that is compatible with its own version. For a `0.x` app that
//! is the same `0.MINOR` line (so `0.2.0` accepts `0.2.1` but not `0.3.0`); for
//! `>=1.0` it is the same major. This keeps an old app from eagerly pulling a
//! breaking new engine: the app updates itself first, then reconciles the engine
//! to the range the new app understands.

use anyhow::{Context, Result};
use semver::{Version, VersionReq};
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
    pub model_manifest: AssetName,
    // The app-shell installer asset. The app self-update path consumes this via
    // `latest.json` (the Tauri updater), not from here; kept for completeness.
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

#[derive(Debug, Clone, Deserialize)]
struct GhRelease {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<GhAsset>,
}

#[derive(Debug, Clone, Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
}

/// A resolved release: its tag, its parsed `release.json`, and the asset map
/// used to resolve download URLs by filename.
pub struct ResolvedRelease {
    pub tag: String,
    pub manifest: ReleaseManifest,
    assets: Vec<GhAsset>,
}

impl ResolvedRelease {
    /// The `browser_download_url` for an asset filename, if present.
    pub fn asset_url(&self, name: &str) -> Option<String> {
        self.assets
            .iter()
            .find(|asset| asset.name == name)
            .map(|asset| asset.browser_download_url.clone())
    }
}

/// Strip a leading `v` so `v0.1.0` and `0.1.0` compare equal.
pub fn normalize_version(version: &str) -> &str {
    version.strip_prefix('v').unwrap_or(version)
}

/// Parse a release tag or `--version` string into a semver `Version`.
///
/// Accepts a leading `v`. Git-describe strings like `0.2.0-3-gabc123` parse with
/// the trailing commit info as prerelease metadata; callers that only care about
/// the release line use the `major.minor.patch` core.
pub fn parse_version(text: &str) -> Option<Version> {
    Version::parse(normalize_version(text)).ok()
}

/// The caret requirement an app build uses to pick a compatible engine.
///
/// Returns `None` when the app version is the `0.0.0` placeholder (an untagged
/// local build) or otherwise unparseable; callers then fall back to the latest
/// release so local/dev installs still work. For a real version this is the
/// standard caret range of its `major.minor.patch` core: `^0.2.0` accepts
/// `>=0.2.0, <0.3.0`, while `^1.2.0` accepts `>=1.2.0, <2.0.0`.
pub fn engine_requirement(app_version: &str) -> Option<VersionReq> {
    let v = parse_version(app_version)?;
    if v.major == 0 && v.minor == 0 && v.patch == 0 {
        return None;
    }
    VersionReq::parse(&format!("^{}.{}.{}", v.major, v.minor, v.patch)).ok()
}

/// Resolve the engine release this app build should run.
///
/// In order of preference: the newest published, non-draft, non-prerelease
/// release whose tag satisfies the app's caret range; then, as a last resort, the
/// release whose tag equals the app's own version exactly. That last step is what
/// lets a prerelease app reach its corresponding prerelease engine: prereleases
/// are excluded by the caret/non-prerelease rules above, so a `0.3.0-rc.1` app
/// would otherwise find nothing, and instead pins to the `v0.3.0-rc.1` engine.
///
/// For the `0.0.0` placeholder (untagged local build) it falls back to the latest
/// release so dev installs still work.
pub fn resolve_engine_release(app_version: &str) -> Result<Option<ResolvedRelease>> {
    if engine_requirement(app_version).is_none() {
        // Untagged local build: no caret pin, use the latest release.
        return Ok(Some(fetch_latest()?));
    }
    let releases = list_releases()?;
    for rel in engine_candidates(app_version, &releases) {
        if let Some(resolved) = resolve_manifest(rel.clone())? {
            return Ok(Some(resolved));
        }
    }
    Ok(None)
}

/// The releases to try for this app version, best first: newest compatible
/// (caret, non-draft, non-prerelease) releases, then the release matching the
/// app's own version exactly as a fallback (prerelease-friendly). Pure so the
/// selection order is unit tested without the network.
fn engine_candidates<'a>(app_version: &str, releases: &'a [GhRelease]) -> Vec<&'a GhRelease> {
    let mut out: Vec<&GhRelease> = Vec::new();

    if let Some(req) = engine_requirement(app_version) {
        let mut compatible: Vec<(Version, &GhRelease)> = releases
            .iter()
            .filter(|rel| !rel.draft && !rel.prerelease)
            .filter_map(|rel| parse_version(&rel.tag_name).map(|v| (v, rel)))
            .filter(|(v, _)| req.matches(v))
            .collect();
        // Newest compatible first.
        compatible.sort_by(|a, b| b.0.cmp(&a.0));
        out.extend(compatible.into_iter().map(|(_, rel)| rel));
    }

    // Fallback: the release whose tag is exactly the app's own version, including
    // prereleases. Drafts stay excluded (they have no public download URL).
    if let Some(app_v) = parse_version(app_version) {
        if let Some(rel) = releases
            .iter()
            .find(|rel| !rel.draft && parse_version(&rel.tag_name).as_ref() == Some(&app_v))
        {
            if !out.iter().any(|existing| existing.tag_name == rel.tag_name) {
                out.push(rel);
            }
        }
    }

    out
}

/// Resolve the latest published release and its manifest. Used as the dev/local
/// fallback when the app version cannot be pinned to a caret range.
pub fn fetch_latest() -> Result<ResolvedRelease> {
    let api = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let release: GhRelease =
        download::get_json(&api).with_context(|| format!("querying latest release for {REPO}"))?;
    resolve_manifest(release)?.context("latest release is missing release.json")
}

/// All releases on the first page (newest first), with their assets.
fn list_releases() -> Result<Vec<GhRelease>> {
    let api = format!("https://api.github.com/repos/{REPO}/releases?per_page=100");
    download::get_json(&api).with_context(|| format!("listing releases for {REPO}"))
}

/// Fetch and parse a release's `release.json`, returning `None` when the release
/// has no such asset (so the caller can fall through to an older candidate).
fn resolve_manifest(release: GhRelease) -> Result<Option<ResolvedRelease>> {
    let Some(manifest_url) = release
        .assets
        .iter()
        .find(|asset| asset.name == "release.json")
        .map(|asset| asset.browser_download_url.clone())
    else {
        return Ok(None);
    };
    let manifest: ReleaseManifest =
        download::get_json(&manifest_url).context("parsing release.json")?;
    Ok(Some(ResolvedRelease {
        tag: release.tag_name,
        manifest,
        assets: release.assets,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rel(tag: &str, draft: bool, prerelease: bool, with_manifest: bool) -> GhRelease {
        let assets = if with_manifest {
            vec![GhAsset {
                name: "release.json".to_string(),
                browser_download_url: format!("https://example/{tag}/release.json"),
            }]
        } else {
            vec![]
        };
        GhRelease {
            tag_name: tag.to_string(),
            draft,
            prerelease,
            assets,
        }
    }

    /// The tag `resolve_engine_release` would settle on: the first of the ordered
    /// `engine_candidates` that actually has a `release.json`.
    fn pick(app_version: &str, releases: &[GhRelease]) -> Option<String> {
        engine_candidates(app_version, releases)
            .into_iter()
            .find(|r| r.assets.iter().any(|a| a.name == "release.json"))
            .map(|r| r.tag_name.clone())
    }

    #[test]
    fn parses_release_manifest() {
        let json = r#"{
            "version": "v0.1.0",
            "cli": { "asset": "mite.exe", "sha256": "abc" },
            "modelManifest": { "asset": "model-manifest.json" },
            "installer": { "asset": "mite-setup.exe", "sha256": "ghi" }
        }"#;
        let manifest: ReleaseManifest = serde_json::from_str(json).unwrap();
        assert_eq!(manifest.version, "v0.1.0");
        assert_eq!(manifest.cli.asset, "mite.exe");
        assert_eq!(manifest.model_manifest.asset, "model-manifest.json");
        assert_eq!(manifest.installer.unwrap().sha256, "ghi");
    }

    #[test]
    fn normalizes_versions() {
        assert_eq!(normalize_version("v0.1.0"), "0.1.0");
        assert_eq!(normalize_version("0.1.0"), "0.1.0");
    }

    #[test]
    fn parses_versions_with_and_without_prefix() {
        assert_eq!(parse_version("v0.2.0"), Some(Version::new(0, 2, 0)));
        assert_eq!(parse_version("0.2.1"), Some(Version::new(0, 2, 1)));
        // git-describe form keeps the release core.
        let v = parse_version("0.2.0-3-gabc123").unwrap();
        assert_eq!((v.major, v.minor, v.patch), (0, 2, 0));
        assert_eq!(parse_version("not-a-version"), None);
    }

    #[test]
    fn caret_requirement_follows_semver_zero_rules() {
        // 0.x: same 0.MINOR line is compatible; the next minor is breaking.
        let req = engine_requirement("v0.2.0").unwrap();
        assert!(req.matches(&Version::new(0, 2, 0)));
        assert!(req.matches(&Version::new(0, 2, 1)));
        assert!(req.matches(&Version::new(0, 2, 9)));
        assert!(!req.matches(&Version::new(0, 3, 0)));
        assert!(!req.matches(&Version::new(0, 1, 9)));

        // >=1.0: same major is compatible up to the next major.
        let req = engine_requirement("1.2.0").unwrap();
        assert!(req.matches(&Version::new(1, 2, 0)));
        assert!(req.matches(&Version::new(1, 5, 3)));
        assert!(!req.matches(&Version::new(2, 0, 0)));
        assert!(!req.matches(&Version::new(1, 1, 0)));
    }

    #[test]
    fn placeholder_version_has_no_requirement() {
        // Untagged local build: no caret pin, callers fall back to latest.
        assert!(engine_requirement("0.0.0").is_none());
        assert!(engine_requirement("garbage").is_none());
    }

    #[test]
    fn picks_newest_compatible_release() {
        let releases = vec![
            rel("v0.3.0", false, false, true),
            rel("v0.2.2", false, false, true),
            rel("v0.2.1", false, false, true),
            rel("v0.2.0", false, false, true),
            rel("v0.1.0", false, false, true),
        ];
        // App on 0.2.0 takes the newest 0.2.x, never the breaking 0.3.0.
        assert_eq!(pick("v0.2.0", &releases).as_deref(), Some("v0.2.2"));
        // App on 0.1.0 is stuck at its line even though newer exists.
        assert_eq!(pick("v0.1.0", &releases).as_deref(), Some("v0.1.0"));
        // App on 0.3.0 takes 0.3.0.
        assert_eq!(pick("v0.3.0", &releases).as_deref(), Some("v0.3.0"));
    }

    #[test]
    fn skips_drafts_prereleases_and_manifestless_releases() {
        let releases = vec![
            rel("v0.2.3", true, false, true),   // draft
            rel("v0.2.2", false, true, true),   // prerelease
            rel("v0.2.1", false, false, false), // no release.json
            rel("v0.2.0", false, false, true),  // usable
        ];
        assert_eq!(pick("v0.2.0", &releases).as_deref(), Some("v0.2.0"));
    }

    #[test]
    fn no_compatible_release_yields_none() {
        // 0.3.0 is out of ^0.2.0, and there is no release tagged exactly 0.2.0
        // to fall back to.
        let releases = vec![rel("v0.3.0", false, false, true)];
        assert_eq!(pick("v0.2.0", &releases), None);
    }

    #[test]
    fn prerelease_app_falls_back_to_its_own_prerelease_engine() {
        let releases = vec![
            rel("v0.3.0-rc.1", false, true, true), // prerelease engine
            rel("v0.2.0", false, false, true),     // newest stable
        ];
        // The caret/non-prerelease rules find nothing for an rc app, so it pins
        // to its own exact prerelease engine rather than a stale stable one.
        assert_eq!(
            pick("v0.3.0-rc.1", &releases).as_deref(),
            Some("v0.3.0-rc.1")
        );
    }

    #[test]
    fn own_version_fallback_only_when_no_compatible_release() {
        // When a compatible stable release exists, the exact-own-version match is
        // the same release and not a separate fallback: a 0.2.0 app on a feed with
        // 0.2.1 still prefers the newest compatible.
        let releases = vec![
            rel("v0.2.1", false, false, true),
            rel("v0.2.0", false, false, true),
        ];
        assert_eq!(pick("v0.2.0", &releases).as_deref(), Some("v0.2.1"));
    }

    #[test]
    fn own_version_fallback_skips_drafts() {
        // A draft tagged like the app's own version is not publicly downloadable,
        // so it is not a usable fallback.
        let releases = vec![rel("v0.3.0-rc.1", true, true, true)];
        assert_eq!(pick("v0.3.0-rc.1", &releases), None);
    }
}
