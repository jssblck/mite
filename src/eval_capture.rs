//! Raw frame captures for building real-game eval fixtures.
//!
//! These captures intentionally stop after the active `FrameSource` produces a
//! frame. They do not run OCR, dictionary lookup, overlay presentation, or any
//! smoothing reuse policy, so the PNG is the exact raw frame Mite would have fed
//! into the OCR pipeline.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::artifact::{ARTIFACT_VERSION, unix_ms, write_json_pretty};
use crate::capture::{Frame, FrameSourceMetadata};
use crate::geometry::ScreenRect;

const CAPTURE_ROOT_DIR: &str = "mite";
const CAPTURE_SUBDIR: &str = "eval-captures";
const CAPTURE_DIR_PREFIX: &str = "capture-";
const RAW_IMAGE_NAME: &str = "underlying.png";
const META_FILE_NAME: &str = "capture.json";

/// A compact signature of one OCR detection's recognized text and box layout.
///
/// It is embedded in `capture.json` for automatic eval captures so a later
/// `watch` session can skip re-saving a scene it has already captured: the same
/// similarity score that decides "this is a new scene" at runtime is replayed
/// against the fingerprints already on disk. Manual hotkey captures (a raw
/// frame with no OCR) carry no fingerprint, so this stays `None` for them.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DetectionFingerprint {
    /// Normalized recognized line strings, sorted and deduped.
    pub lines: Vec<String>,
    /// Quantized box rects as `[x, y, width, height]` buckets, sorted and
    /// deduped.
    pub boxes: Vec<[i32; 4]>,
}

#[derive(Serialize)]
struct RawCaptureMeta<'a> {
    artifact_version: u32,
    capture_kind: &'static str,
    captured_unix_ms: u128,
    window_id: u32,
    screen_rect: ScreenRect,
    raw_image: &'static str,
    frame_id: u64,
    content_epoch: u64,
    frames_delivered: u32,
    staging_age_ms: f64,
    source: RawCaptureSource<'a>,
    #[serde(skip_serializing_if = "Option::is_none")]
    detection_fingerprint: Option<&'a DetectionFingerprint>,
}

#[derive(Serialize)]
struct RawCaptureSource<'a> {
    kind: &'static str,
    label: Option<&'a str>,
    app_name: Option<&'a str>,
    window_id: Option<u32>,
    pid: Option<u32>,
}

impl<'a> From<&'a FrameSourceMetadata> for RawCaptureSource<'a> {
    fn from(source: &'a FrameSourceMetadata) -> Self {
        Self {
            kind: source.kind.as_str(),
            label: source.label.as_deref(),
            app_name: source.app_name.as_deref(),
            window_id: source.window_id,
            pid: source.pid,
        }
    }
}

pub fn default_capture_root() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join(CAPTURE_ROOT_DIR).join(CAPTURE_SUBDIR)
}

pub fn write_raw_capture(
    root: &Path,
    window_id: u32,
    frame: &Frame,
    detection_fingerprint: Option<&DetectionFingerprint>,
) -> Result<PathBuf> {
    let image = frame
        .pixels
        .as_ref()
        .context("captured frame did not retain pixels")?;
    if frame.size.is_empty() {
        bail!("captured frame is empty");
    }

    fs::create_dir_all(root).with_context(|| format!("failed to create {}", root.display()))?;
    let captured_unix_ms = unix_ms();
    let dir = unique_capture_dir(root, captured_unix_ms)?;

    let raw = dir.join(RAW_IMAGE_NAME);
    image
        .save(&raw)
        .with_context(|| format!("failed to save {}", raw.display()))?;

    let meta = RawCaptureMeta {
        artifact_version: ARTIFACT_VERSION,
        capture_kind: "raw_eval_frame",
        captured_unix_ms,
        window_id,
        screen_rect: frame.screen_rect,
        raw_image: RAW_IMAGE_NAME,
        frame_id: frame.id,
        content_epoch: frame.content_epoch,
        frames_delivered: frame.frames_delivered,
        staging_age_ms: frame.staging_age.as_secs_f64() * 1000.0,
        source: RawCaptureSource::from(&frame.source),
        detection_fingerprint,
    };
    write_json_pretty(&dir.join(META_FILE_NAME), &meta)?;

    Ok(dir)
}

/// Load the detection fingerprints recorded by previous automatic captures
/// under `root`. Used to dedup across `watch` sessions so a scene already saved
/// is not captured again. Best-effort: fingerprint-less captures (including
/// every manual hotkey capture) and non-capture directories are skipped
/// quietly, and a missing root yields an empty list. Real failures (a readable
/// root that cannot be enumerated, an unreadable existing `capture.json`, or
/// corrupt metadata) are logged, because they silently shrink the dedup set and
/// would otherwise let already-saved scenes be captured again with no warning.
pub fn load_existing_fingerprints(root: &Path) -> Vec<DetectionFingerprint> {
    #[derive(Deserialize)]
    struct Stored {
        #[serde(default)]
        detection_fingerprint: Option<DetectionFingerprint>,
    }

    let mut fingerprints = Vec::new();
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        // A missing root is the normal first-run case. Any other error (a
        // permission denial or I/O fault on an existing dir) disables
        // cross-session dedup, so surface it instead of returning empty.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return fingerprints,
        Err(error) => {
            tracing::warn!(
                "cannot read eval-capture dir {} for dedup; cross-session dedup disabled: {error:#}",
                root.display()
            );
            return fingerprints;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                tracing::warn!("skipping an eval-capture entry during dedup scan: {error:#}");
                continue;
            }
        };
        match entry.file_type() {
            Ok(file_type) if file_type.is_dir() => {}
            Ok(_) => continue,
            Err(error) => {
                tracing::warn!(
                    "skipping {} during dedup scan (cannot stat): {error:#}",
                    entry.path().display()
                );
                continue;
            }
        }
        let meta_path = entry.path().join(META_FILE_NAME);
        let text = match fs::read_to_string(&meta_path) {
            Ok(text) => text,
            // No capture.json means this is not a capture bundle: skip quietly.
            // Any other read error is an existing capture we cannot dedup on.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                tracing::warn!(
                    "skipping {} during dedup scan (unreadable): {error:#}",
                    meta_path.display()
                );
                continue;
            }
        };
        match serde_json::from_str::<Stored>(&text) {
            Ok(stored) => {
                if let Some(fingerprint) = stored.detection_fingerprint {
                    fingerprints.push(fingerprint);
                }
            }
            Err(error) => tracing::warn!(
                "skipping {} during dedup scan (corrupt metadata): {error:#}",
                meta_path.display()
            ),
        }
    }
    fingerprints
}

fn unique_capture_dir(root: &Path, captured_unix_ms: u128) -> Result<PathBuf> {
    for suffix in std::iter::once(String::new()).chain((1..1000).map(|n| format!("-{n}"))) {
        let dir = root.join(format!("{CAPTURE_DIR_PREFIX}{captured_unix_ms}{suffix}"));
        match fs::create_dir(&dir) {
            Ok(()) => return Ok(dir),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(error).with_context(|| format!("failed to create {}", dir.display()));
            }
        }
    }
    bail!(
        "failed to allocate a unique eval capture directory under {}",
        root.display()
    )
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use image::{Rgb, RgbImage};
    use serde_json::Value;

    use super::*;
    use crate::capture::{FrameSourceKind, FrameSourceMetadata};
    use crate::geometry::Size;

    #[test]
    fn writes_raw_png_and_metadata_without_ocr_fields() {
        let root = tempfile::tempdir().unwrap();
        let image = RgbImage::from_pixel(2, 1, Rgb([10, 20, 30]));
        let frame = Frame {
            id: 7,
            captured_at: Instant::now(),
            size: Size::new(2, 1),
            screen_rect: ScreenRect::new(100, 200, Size::new(2, 1)),
            source: FrameSourceMetadata {
                kind: FrameSourceKind::WindowsGraphicsCapture,
                label: Some("Game".to_string()),
                app_name: Some("game.exe".to_string()),
                window_id: Some(42),
                pid: Some(99),
            },
            content_epoch: 123,
            pixels: Some(std::sync::Arc::new(image)),
            frames_delivered: 3,
            staging_age: Duration::from_millis(12),
        };

        let dir = write_raw_capture(root.path(), 42, &frame, None).unwrap();

        assert!(dir.join("underlying.png").exists());
        let meta: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("capture.json")).unwrap())
                .unwrap();
        assert_eq!(meta["capture_kind"], "raw_eval_frame");
        assert_eq!(meta["window_id"], 42);
        assert_eq!(meta["raw_image"], "underlying.png");
        assert_eq!(meta["source"]["kind"], "windows_graphics_capture");
        assert!(meta.get("lines").is_none());
        assert!(meta.get("words").is_none());
        // A manual capture (no fingerprint) omits the field entirely.
        assert!(meta.get("detection_fingerprint").is_none());
    }

    #[test]
    fn embeds_and_reloads_detection_fingerprint() {
        let root = tempfile::tempdir().unwrap();
        let image = RgbImage::from_pixel(2, 1, Rgb([10, 20, 30]));
        let frame = Frame {
            id: 1,
            captured_at: Instant::now(),
            size: Size::new(2, 1),
            screen_rect: ScreenRect::new(0, 0, Size::new(2, 1)),
            source: FrameSourceMetadata {
                kind: FrameSourceKind::WindowsGraphicsCapture,
                label: None,
                app_name: None,
                window_id: None,
                pid: None,
            },
            content_epoch: 0,
            pixels: Some(std::sync::Arc::new(image)),
            frames_delivered: 1,
            staging_age: Duration::ZERO,
        };
        let fingerprint = DetectionFingerprint {
            lines: vec!["こんにちは".to_string()],
            boxes: vec![[1, 2, 3, 4]],
        };

        let dir = write_raw_capture(root.path(), 7, &frame, Some(&fingerprint)).unwrap();
        let meta: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("capture.json")).unwrap())
                .unwrap();
        assert_eq!(meta["detection_fingerprint"]["lines"][0], "こんにちは");

        let loaded = load_existing_fingerprints(root.path());
        assert_eq!(loaded, vec![fingerprint]);
    }

    #[test]
    fn load_existing_fingerprints_is_empty_for_missing_root() {
        let root = tempfile::tempdir().unwrap();
        let missing = root.path().join("does-not-exist");
        assert!(load_existing_fingerprints(&missing).is_empty());
    }

    #[test]
    fn load_existing_fingerprints_skips_corrupt_and_partial_captures_keeping_good_ones() {
        let root = tempfile::tempdir().unwrap();

        // A valid fingerprinted capture: the one we expect back.
        let image = RgbImage::from_pixel(2, 1, Rgb([10, 20, 30]));
        let frame = Frame {
            id: 1,
            captured_at: Instant::now(),
            size: Size::new(2, 1),
            screen_rect: ScreenRect::new(0, 0, Size::new(2, 1)),
            source: FrameSourceMetadata {
                kind: FrameSourceKind::WindowsGraphicsCapture,
                label: None,
                app_name: None,
                window_id: Some(1),
                pid: None,
            },
            content_epoch: 1,
            pixels: Some(std::sync::Arc::new(image)),
            frames_delivered: 1,
            staging_age: Duration::from_millis(0),
        };
        let fingerprint = DetectionFingerprint {
            lines: vec!["ありがとう".to_string()],
            boxes: vec![[1, 2, 3, 4]],
        };
        write_raw_capture(root.path(), 1, &frame, Some(&fingerprint)).unwrap();

        // A manual-style capture with no fingerprint: skipped quietly.
        write_raw_capture(root.path(), 2, &frame, None).unwrap();

        // A directory whose capture.json is corrupt: skipped (with a warning),
        // not a panic, and it must not abort the scan of the good capture.
        let corrupt = root.path().join("capture-3");
        fs::create_dir(&corrupt).unwrap();
        fs::write(corrupt.join(META_FILE_NAME), b"{ this is not json").unwrap();

        // A stray non-capture directory with no metadata: skipped quietly.
        fs::create_dir(root.path().join("notes")).unwrap();

        let loaded = load_existing_fingerprints(root.path());
        assert_eq!(loaded, vec![fingerprint]);
    }
}
