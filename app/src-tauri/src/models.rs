//! The model/dictionary manifest and the download plan that fills `models\`.
//!
//! Schema and behavior mirror the repo's `model-manifest.json` and
//! `scripts\bootstrap-dev.ps1`: direct files verify their SHA256 against the
//! downloaded file, archived lexicons extract a single member and verify the
//! extracted file, and directory archives (frequency data) extract in full with
//! no checksum.

use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::download;

#[derive(Debug, Deserialize)]
pub struct ModelManifest {
    #[serde(default)]
    pub models: Vec<ModelEntry>,
    #[serde(default)]
    pub lexicons: Vec<LexiconEntry>,
    #[serde(default)]
    pub frequencies: Vec<FrequencyEntry>,
}

#[derive(Debug, Deserialize)]
pub struct ModelEntry {
    pub id: String,
    pub local_path: String,
    pub url: String,
    #[serde(default)]
    pub sha256: Option<String>,
    #[serde(default)]
    pub optional: bool,
}

#[derive(Debug, Deserialize)]
pub struct LexiconEntry {
    pub id: String,
    pub local_path: String,
    pub archive: Archive,
    #[serde(default)]
    pub sha256: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct FrequencyEntry {
    pub id: String,
    pub local_path: String,
    pub archive: Archive,
}

#[derive(Debug, Deserialize)]
pub struct Archive {
    pub url: String,
    #[serde(default)]
    pub member_glob: Option<String>,
}

/// Download everything the manifest requires into `home`, skipping optional
/// models and anything already present. `progress(id, received, total, done)`
/// fires as bytes arrive and once per asset on completion.
pub fn download_all(
    home: &Path,
    manifest: &ModelManifest,
    mut progress: impl FnMut(&str, u64, u64, bool),
) -> Result<()> {
    for model in &manifest.models {
        if model.optional {
            continue;
        }
        let target = home.join(&model.local_path);
        if target.exists() {
            progress(&model.id, 1, 1, true);
            continue;
        }
        download::download_to_file(&model.url, &target, |received, total| {
            progress(&model.id, received, total, false)
        })
        .with_context(|| format!("downloading {}", model.id))?;
        if let Some(sha) = &model.sha256 {
            download::verify_sha256(&target, sha)?;
        }
        progress(&model.id, 1, 1, true);
    }

    for lexicon in &manifest.lexicons {
        let target = home.join(&lexicon.local_path);
        if target.exists() {
            progress(&lexicon.id, 1, 1, true);
            continue;
        }
        let glob =
            lexicon.archive.member_glob.as_deref().with_context(|| {
                format!("lexicon {} is missing archive.member_glob", lexicon.id)
            })?;
        let tmp = home.join(format!("{}.zip.tmp", lexicon.id));
        download::download_to_file(&lexicon.archive.url, &tmp, |received, total| {
            progress(&lexicon.id, received, total, false)
        })
        .with_context(|| format!("downloading {}", lexicon.id))?;
        download::extract_zip_member(&tmp, glob, &target)?;
        if let Some(sha) = &lexicon.sha256 {
            download::verify_sha256(&target, sha)?;
        }
        let _ = std::fs::remove_file(&tmp);
        progress(&lexicon.id, 1, 1, true);
    }

    for frequency in &manifest.frequencies {
        let target = home.join(&frequency.local_path);
        if target.exists() {
            progress(&frequency.id, 1, 1, true);
            continue;
        }
        let tmp = home.join(format!("{}.zip.tmp", frequency.id));
        download::download_to_file(&frequency.archive.url, &tmp, |received, total| {
            progress(&frequency.id, received, total, false)
        })
        .with_context(|| format!("downloading {}", frequency.id))?;
        download::extract_zip_all(&tmp, &target)?;
        let _ = std::fs::remove_file(&tmp);
        progress(&frequency.id, 1, 1, true);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_repo_manifest_shape() {
        let json = r#"{
            "models": [
                { "id": "det", "kind": "detector", "local_path": "models/det.onnx",
                  "url": "https://example/det.onnx", "sha256": "aa" },
                { "id": "srv", "kind": "detector", "local_path": "models/srv.onnx",
                  "url": "https://example/srv.onnx", "sha256": "bb", "optional": true }
            ],
            "lexicons": [
                { "id": "jmdict-eng", "kind": "lexicon", "local_path": "models/jmdict-eng.json",
                  "archive": { "format": "zip", "url": "https://example/j.zip",
                  "member_glob": "jmdict-eng-*.json" }, "sha256": "cc" }
            ],
            "frequencies": [
                { "id": "jpdb-freq", "kind": "frequency", "local_path": "models/jpdb-freq",
                  "archive": { "format": "zip", "url": "https://example/f.zip" } }
            ]
        }"#;
        let manifest: ModelManifest = serde_json::from_str(json).unwrap();
        assert_eq!(manifest.models.len(), 2);
        assert_eq!(manifest.models.iter().filter(|m| !m.optional).count(), 1);
        assert_eq!(
            manifest.lexicons[0].archive.member_glob.as_deref(),
            Some("jmdict-eng-*.json")
        );
    }
}
