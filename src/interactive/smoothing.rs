//! Temporal smoothing for `watch`.
//!
//! Rather than a whole-frame signature (which a 3D game's animated background
//! changes every frame), this samples luma only inside the previously detected
//! text regions. Text on a stable UI panel stays put even as the background
//! animates, so this trips on real text changes but tolerates surrounding motion.

use std::time::Instant;

use image::RgbImage;

use crate::geometry::Rect;
use crate::hover::WordSpan;
use crate::ocr::RecognizedText;

const SIG_PTS_X: u32 = 8;
const SIG_PTS_Y: u32 = 4;
/// Max mean per-sample luma difference (0-255) within the text regions still
/// considered the same scene.
const SCENE_STABLE_THRESHOLD: u32 = 6;

/// Integer Rec601 luma weights (R, G, B) summing to 256, paired with a
/// right-shift by [`LUMA_SHIFT`] - a cheap luma approximation for the region
/// signature.
const INTEGER_LUMA_WEIGHTS: [u32; 3] = [77, 150, 29];
const LUMA_SHIFT: u32 = 8;

/// The text rects sampled at the last full detection, paired with their luma, so
/// the next frame can be sampled at the same points and compared.
pub(super) struct Anchor {
    rects: Vec<Rect>,
    luma: Vec<u8>,
}

impl Anchor {
    pub(super) fn from_detection(image: &RgbImage, rects: Vec<Rect>) -> Self {
        let luma = region_signature(image, &rects);
        Self { rects, luma }
    }

    /// Whether `image` still looks like the anchored scene when sampled at the
    /// same text regions (tolerating animated backgrounds outside them).
    pub(super) fn matches(&self, image: &RgbImage) -> bool {
        signature_diff(&self.luma, &region_signature(image, &self.rects)) <= SCENE_STABLE_THRESHOLD
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

/// Sample a small luma grid inside each rect (cell centres), concatenated. Used
/// as a region-targeted scene signature: sampling the same rects on a later
/// frame yields a comparable vector.
fn region_signature(image: &RgbImage, rects: &[Rect]) -> Vec<u8> {
    let (width, height) = (image.width(), image.height());
    let mut signature = Vec::with_capacity(rects.len() * (SIG_PTS_X * SIG_PTS_Y) as usize);
    for rect in rects {
        for py in 0..SIG_PTS_Y {
            let fy = rect.y + (py as f32 + 0.5) / SIG_PTS_Y as f32 * rect.height;
            let y = (fy as i32).clamp(0, height as i32 - 1) as u32;
            for px in 0..SIG_PTS_X {
                let fx = rect.x + (px as f32 + 0.5) / SIG_PTS_X as f32 * rect.width;
                let x = (fx as i32).clamp(0, width as i32 - 1) as u32;
                let p = image.get_pixel(x, y);
                let luma = (p[0] as u32 * INTEGER_LUMA_WEIGHTS[0]
                    + p[1] as u32 * INTEGER_LUMA_WEIGHTS[1]
                    + p[2] as u32 * INTEGER_LUMA_WEIGHTS[2])
                    >> LUMA_SHIFT;
                signature.push(luma as u8);
            }
        }
    }
    signature
}

/// Mean absolute per-sample luma difference between two signatures (0-255);
/// `u32::MAX` if they are incomparable.
fn signature_diff(a: &[u8], b: &[u8]) -> u32 {
    if a.is_empty() || a.len() != b.len() {
        return u32::MAX;
    }
    let sum: u32 = a.iter().zip(b).map(|(x, y)| x.abs_diff(*y) as u32).sum();
    sum / a.len() as u32
}
