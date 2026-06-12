//! Debug captures for the interactive overlay.
//!
//! When the problem-report button in the popup is clicked, this writes a
//! self-contained folder under the user's local data directory containing the
//! OCR'd window frame, that frame with the overlay composited on top, and a
//! JSON dump of everything the lookup produced — so an issue seen in a game can
//! be filed and discussed against concrete artifacts.

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use image::RgbImage;
use serde::Serialize;

use crate::artifact::{ARTIFACT_VERSION, unix_ms, write_json_pretty};
use crate::geometry::ScreenRect;
use crate::hover::WordSpan;
use crate::ocr::RecognizedText;

/// Filenames written into each debug-capture folder.
const UNDERLYING_IMAGE_NAME: &str = "underlying.png";
const OVERLAY_IMAGE_NAME: &str = "with_overlay.png";
const META_FILE_NAME: &str = "capture.json";
/// Prefix for the per-capture folder name (followed by the unix-ms timestamp).
const CAPTURE_DIR_PREFIX: &str = "capture-";
/// Local-data subdirectories under `%LOCALAPPDATA%` (or the temp dir) where
/// captures are written.
const CAPTURE_ROOT_DIR: &str = "mite";
const CAPTURE_SUBDIR: &str = "debug-captures";

/// Everything needed to write one debug capture.
pub struct CaptureInput<'a> {
    /// The OCR'd window frame (the "underlying" image).
    pub frame: &'a RgbImage,
    /// The overlay's premultiplied BGRA surface (same size as the frame).
    pub overlay_width: i32,
    pub overlay_height: i32,
    pub overlay_bgra: &'a [u8],
    pub screen_rect: ScreenRect,
    pub window_id: u32,
    /// Raw OCR lines (text, box, confidence, per-glyph centres).
    pub lines: &'a [RecognizedText],
    /// Per-word lookup results.
    pub words: &'a [WordSpan],
}

#[derive(Serialize)]
struct CaptureMeta<'a> {
    artifact_version: u32,
    captured_unix_ms: u128,
    window_id: u32,
    screen_rect: ScreenRect,
    underlying_image: &'a str,
    overlay_image: &'a str,
    lines: &'a [RecognizedText],
    words: &'a [WordSpan],
}

/// Write a debug capture folder and return its path.
pub fn write_capture(input: &CaptureInput) -> Result<PathBuf> {
    let captured_unix_ms = unix_ms();
    let dir = captures_dir().join(format!("{CAPTURE_DIR_PREFIX}{captured_unix_ms}"));
    fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;

    let underlying = dir.join(UNDERLYING_IMAGE_NAME);
    input
        .frame
        .save(&underlying)
        .with_context(|| format!("failed to save {}", underlying.display()))?;

    let composited = composite(
        input.frame,
        input.overlay_width,
        input.overlay_height,
        input.overlay_bgra,
    );
    let overlay_path = dir.join(OVERLAY_IMAGE_NAME);
    composited
        .save(&overlay_path)
        .with_context(|| format!("failed to save {}", overlay_path.display()))?;

    let meta = CaptureMeta {
        artifact_version: ARTIFACT_VERSION,
        captured_unix_ms,
        window_id: input.window_id,
        screen_rect: input.screen_rect,
        underlying_image: UNDERLYING_IMAGE_NAME,
        overlay_image: OVERLAY_IMAGE_NAME,
        lines: input.lines,
        words: input.words,
    };
    write_json_pretty(&dir.join(META_FILE_NAME), &meta)?;

    Ok(dir)
}

/// Alpha-composite the overlay's premultiplied BGRA surface over the frame.
fn composite(frame: &RgbImage, overlay_w: i32, overlay_h: i32, bgra: &[u8]) -> RgbImage {
    let mut out = frame.clone();
    let width = (frame.width() as i32).min(overlay_w).max(0) as u32;
    let height = (frame.height() as i32).min(overlay_h).max(0) as u32;
    let stride = overlay_w.max(0) as usize * 4;

    for y in 0..height {
        let row = y as usize * stride;
        for x in 0..width {
            let offset = row + x as usize * 4;
            let Some(pixel) = bgra.get(offset..offset + 4) else {
                continue;
            };
            let (b, g, r, a) = (pixel[0], pixel[1], pixel[2], pixel[3]);
            if a == 0 {
                continue;
            }
            // Premultiplied source over destination: out = src + dst*(1-a).
            let inv = (255 - a) as u32;
            let base = out.get_pixel_mut(x, y);
            base.0[0] = (r as u32 + base.0[0] as u32 * inv / 255).min(255) as u8;
            base.0[1] = (g as u32 + base.0[1] as u32 * inv / 255).min(255) as u8;
            base.0[2] = (b as u32 + base.0[2] as u32 * inv / 255).min(255) as u8;
        }
    }
    out
}

fn captures_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join(CAPTURE_ROOT_DIR).join(CAPTURE_SUBDIR)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    #[test]
    fn composite_blends_overlay_over_frame() {
        // 1x1 black frame; overlay = opaque red (premultiplied, a=255).
        let frame = RgbImage::from_pixel(1, 1, Rgb([0, 0, 0]));
        let bgra = [0u8, 0, 255, 255]; // B,G,R,A = red, opaque
        let out = composite(&frame, 1, 1, &bgra);
        assert_eq!(out.get_pixel(0, 0).0, [255, 0, 0]);

        // Half-alpha red over white → pink-ish.
        let white = RgbImage::from_pixel(1, 1, Rgb([255, 255, 255]));
        let half = [0u8, 0, 128, 128]; // premultiplied red at a=128
        let out = composite(&white, 1, 1, &half);
        // out_r = 128 + 255*(127)/255 = 128 + 127 = 255; out_g = 0 + 255*127/255 = 127.
        assert_eq!(out.get_pixel(0, 0).0, [255, 127, 127]);
    }
}
