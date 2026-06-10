//! Temporal smoothing for `watch`.
//!
//! Rather than a whole-frame signature (which a 3D game's animated background
//! changes every frame), this samples luma only inside the previously detected
//! text regions. Text on a stable UI panel stays put even as the background
//! animates, so this trips on real text changes but tolerates surrounding motion.

use std::time::Instant;

use image::RgbImage;

use crate::capture::FrameProbe;
use crate::geometry::{Rect, Size};
use crate::hover::WordSpan;
use crate::ocr::RecognizedText;

const SIG_PTS_X: u32 = 8;
const SIG_PTS_Y: u32 = 4;
/// Max mean per-sample luma difference (0-255) within the text regions still
/// considered the same scene.
const SCENE_STABLE_THRESHOLD: u32 = 6;

/// The text rects sampled at the last full detection, expressed as a
/// [`FrameProbe`] so the same signature can be checked either against a
/// materialized RGB frame or - on the WGC fast path - against the raw staging
/// buffer before any frame is built.
pub(super) struct Anchor {
    probe: FrameProbe,
}

impl Anchor {
    pub(super) fn from_detection(image: &RgbImage, rects: Vec<Rect>) -> Self {
        let size = Size::new(image.width(), image.height());
        let points = signature_points(size, &rects);
        let luma = points
            .iter()
            .map(|&(x, y)| {
                let p = image.get_pixel(x, y);
                FrameProbe::luma(p[0], p[1], p[2])
            })
            .collect();
        Self {
            probe: FrameProbe {
                expected_size: size,
                points,
                luma,
                max_mean_diff: SCENE_STABLE_THRESHOLD,
            },
        }
    }

    /// Whether `image` still looks like the anchored scene when sampled at the
    /// same text regions (tolerating animated backgrounds outside them).
    pub(super) fn matches(&self, image: &RgbImage) -> bool {
        self.probe.matches_rgb(image)
    }

    /// The capture-side form of this anchor, for sources that can answer
    /// "unchanged" before materializing a frame.
    pub(super) fn probe(&self) -> &FrameProbe {
        &self.probe
    }
}

/// A reusable detection result (OCR lines + their word spans) from the last full
/// pass, replayed while the scene is stable.
#[derive(Clone)]
pub(super) struct CachedDetection {
    pub(super) items: Vec<RecognizedText>,
    pub(super) words: Vec<WordSpan>,
}

impl CachedDetection {
    pub(super) fn new(items: Vec<RecognizedText>, words: Vec<WordSpan>) -> Self {
        Self { items, words }
    }
}

/// Worker state carried across passes to reuse detections on a stable scene.
pub(super) struct SmoothingState {
    pub(super) anchor: Option<Anchor>,
    pub(super) cached: Option<CachedDetection>,
    /// When the last full detection ran.
    pub(super) last_full: Instant,
}

impl SmoothingState {
    pub(super) fn new() -> Self {
        Self {
            anchor: None,
            cached: None,
            last_full: Instant::now(),
        }
    }
}

/// A small grid of sample coordinates inside each rect (cell centres),
/// concatenated and clamped in-bounds. Sampling the same points on a later
/// frame yields a comparable signature.
fn signature_points(size: Size, rects: &[Rect]) -> Vec<(u32, u32)> {
    let mut points = Vec::with_capacity(rects.len() * (SIG_PTS_X * SIG_PTS_Y) as usize);
    for rect in rects {
        for py in 0..SIG_PTS_Y {
            let fy = rect.y + (py as f32 + 0.5) / SIG_PTS_Y as f32 * rect.height;
            let y = (fy as i32).clamp(0, size.height as i32 - 1) as u32;
            for px in 0..SIG_PTS_X {
                let fx = rect.x + (px as f32 + 0.5) / SIG_PTS_X as f32 * rect.width;
                let x = (fx as i32).clamp(0, size.width as i32 - 1) as u32;
                points.push((x, y));
            }
        }
    }
    points
}
