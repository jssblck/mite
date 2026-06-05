//! Raw frame captures for building real-game eval fixtures.
//!
//! These captures intentionally stop after the active `FrameSource` produces a
//! frame. They do not run OCR, dictionary lookup, overlay presentation, or any
//! smoothing reuse policy, so the PNG is the exact raw frame Mite would have fed
//! into the OCR pipeline.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Serialize;

use crate::artifact::{ARTIFACT_VERSION, unix_ms, write_json_pretty};
use crate::capture::{Frame, FrameSourceMetadata};
use crate::geometry::ScreenRect;

const CAPTURE_ROOT_DIR: &str = "mite";
const CAPTURE_SUBDIR: &str = "eval-captures";
const CAPTURE_DIR_PREFIX: &str = "capture-";
const RAW_IMAGE_NAME: &str = "underlying.png";
const META_FILE_NAME: &str = "capture.json";

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

pub fn write_raw_capture(root: &Path, window_id: u32, frame: &Frame) -> Result<PathBuf> {
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
    };
    write_json_pretty(&dir.join(META_FILE_NAME), &meta)?;

    Ok(dir)
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
            pixels: Some(image),
            frames_delivered: 3,
            staging_age: Duration::from_millis(12),
        };

        let dir = write_raw_capture(root.path(), 42, &frame).unwrap();

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
    }
}
