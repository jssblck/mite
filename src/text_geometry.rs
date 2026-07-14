//! Pixel-measured text geometry.
//!
//! Text content comes from a human label or the recognizer; this module only
//! measures where that text is rendered, using luminance gradients inside a
//! search rectangle. Eval labels (docs/eval-geometry.md) and the live OCR
//! pipeline both snap their line and character boxes with the same
//! measurement, so runtime output and ground truth share one ruler.

use image::{Rgb, RgbImage};

use crate::{eval::ExpectedDetection, geometry::Rect};

const MIN_LINE_SIDE: u32 = 3;
const ENERGY_FLOOR: f32 = 1.0;
const MIN_CONFIDENCE: f32 = 0.45;

/// Search margins around a detector box before measuring, as fractions of the
/// box height. The DB mask systematically clips weak edge glyphs (trailing
/// 。/：, leading brackets, faint first strokes), so the true extent usually
/// sits slightly outside the detected rect. Horizontal clipping is the common
/// and larger failure; the vertical margin stays smaller so tightly stacked
/// menu rows do not pull a neighboring line into the measured band.
const REFINE_MARGIN_X_RATIO: f32 = 0.45;
const REFINE_MARGIN_Y_RATIO: f32 = 0.30;
const REFINE_MARGIN_MIN_PX: f32 = 4.0;
/// The measured line must cover at least this fraction of the detector box on
/// each axis; below it the measurement locked onto something else entirely
/// (a neighboring label, a UI border) and the detector box is kept.
const REFINE_MIN_AXIS_COVERAGE: f32 = 0.55;
/// Reject measurements that grow the box height beyond this multiple of the
/// detector's height: the band swallowed an adjacent line or UI chrome.
const REFINE_MAX_HEIGHT_GROWTH: f32 = 1.8;
/// Edge-movement caps between the detector edge and the measured edge, as
/// height fractions with px floors; a side that moves further falls back to
/// the detector's edge. Horizontal movement is capped symmetrically: beyond
/// this distance the measurement is as likely to have locked onto an
/// adjacent icon as onto a clipped glyph (the connectivity clamp in
/// `clamp_extent_to_connected` handles chrome closer in). Vertical movement
/// is capped only *outward*: the detector's mask overshoots line height, so
/// growth beyond a few pixels is glow or UI chrome read as ink, while inward
/// shrink onto the glyph band is the correction this stage exists to make
/// and is bounded only by the coverage guard.
const REFINE_MAX_HORIZONTAL_SHIFT_RATIO: f32 = 0.28;
const REFINE_MAX_HORIZONTAL_SHIFT_MIN_PX: f32 = 8.0;
const REFINE_MAX_OUTWARD_SHIFT_RATIO: f32 = 0.10;
const REFINE_MAX_OUTWARD_SHIFT_MIN_PX: f32 = 3.0;
/// Outward growth on *both* horizontal edges beyond this many pixels is
/// bloom or glow around a stylized label, which surrounds the text
/// symmetrically; a genuinely clipped edge glyph extends one side only.
/// Symmetric growth keeps both detector edges. The threshold sits above the
/// 2-3 px antialiasing fringe that ordinary crisp text (and its labels)
/// carries on each side, and below the 5+ px bloom of stylized tabs.
const REFINE_SYMMETRIC_GLOW_PX: f32 = 4.0;
/// A measured extent this close to a search-region edge (px) means the
/// measurement never separated glyph ink from the background: the margins are
/// sized to overshoot the true extent, so a genuine extent ends strictly
/// inside the region. Textured backdrops (game world behind a translucent
/// panel) otherwise read as gradient energy across the whole region and the
/// "extent" runs edge to edge.
const REFINE_REGION_EDGE_EPS: f32 = 3.0;
/// Plausible mean typographic advance per unit of measured line height. CJK
/// advances sit near 1.0-1.5x the ink-band height (kana bands are shorter than
/// their em); outside this range the measured extent spans more (or less) than
/// the recognized text plausibly occupies, e.g. a neighbor glyph captured by
/// the search margin or an ellipsis-only line whose band is a few pixels tall.
const REFINE_MIN_ADVANCE_HEIGHT_RATIO: f32 = 0.25;
const REFINE_MAX_ADVANCE_HEIGHT_RATIO: f32 = 1.9;
/// Hard cap on how far any refined edge may sit outside the detector box.
/// Corpus-scale evidence (12.7k matched detections rescored offline) shows the
/// detector's outer edge is more trustworthy than the measurement's outward
/// extension: refinements growing an edge beyond ~2 px net-regress detection
/// scores, while everything at or inside that fringe nets positive. Two pixels
/// admits the antialiasing fringe a crisp glyph carries without letting glow,
/// bloom, or adjacent chrome pull the box outward.
const REFINE_MAX_OUTWARD_PX: f32 = 2.0;

/// A recognized line's box and per-character x-centres snapped to measured
/// glyph pixels.
#[derive(Debug, Clone, PartialEq)]
pub struct RefinedLineGeometry {
    pub rect: Rect,
    pub char_centers: Vec<f32>,
}

/// Snap a recognized line's detector box to the rendered text's pixel extent.
///
/// The detector box is a search region hint, not trusted geometry: the
/// measurement re-derives the line band and character cells from the original
/// frame exactly like eval label re-annotation does. Returns `None` (keep the
/// detector box) when the measurement is low-confidence or fails the
/// plausibility guards, so refinement can only tighten good reads and never
/// degrades a line the pixels cannot confirm.
pub fn refine_recognized_line(
    image: &RgbImage,
    rect: Rect,
    text: &str,
) -> Option<RefinedLineGeometry> {
    let characters: Vec<String> = text.chars().map(|ch| ch.to_string()).collect();
    // Refuse single-character lines, not just empty ones: with one cell the
    // partition is trivial and its boundary-contrast term is hardcoded to 1.0,
    // so the confidence gate cannot tell a glyph from any nearby decoration,
    // and the advance-plausibility guard has no structure to check either.
    if characters.len() < 2 {
        return None;
    }
    let character_refs: Vec<&str> = characters.iter().map(String::as_str).collect();

    let margin_x = (rect.height * REFINE_MARGIN_X_RATIO).max(REFINE_MARGIN_MIN_PX);
    let margin_y = (rect.height * REFINE_MARGIN_Y_RATIO).max(REFINE_MARGIN_MIN_PX);
    let region = Rect::new(
        rect.x - margin_x,
        rect.y - margin_y,
        rect.width + 2.0 * margin_x,
        rect.height + 2.0 * margin_y,
    );

    let measured = measure_text_geometry_impl(image, region, &character_refs, Some(rect))?;
    let line = measured.line_bounds;

    // A side whose measured edge presses against the search region never
    // separated glyph ink from background there (textured backdrops read as
    // gradient energy across the whole region); keep the detector's edge for
    // that side and the measured edge everywhere else. All four sides failing
    // means the pixels confirmed nothing at all.
    let hugs = region_edge_hugs(line, region, image.width() as f32, image.height() as f32);
    if hugs.all() {
        return None;
    }
    let vertical_outward =
        (rect.height * REFINE_MAX_OUTWARD_SHIFT_RATIO).max(REFINE_MAX_OUTWARD_SHIFT_MIN_PX);
    let horizontal =
        (rect.height * REFINE_MAX_HORIZONTAL_SHIFT_RATIO).max(REFINE_MAX_HORIZONTAL_SHIFT_MIN_PX);
    let symmetric_glow = rect.x - line.x > REFINE_SYMMETRIC_GLOW_PX
        && line.right() - rect.right() > REFINE_SYMMETRIC_GLOW_PX;
    let keep_left = hugs.left || symmetric_glow || (line.x - rect.x).abs() > horizontal;
    let keep_right =
        hugs.right || symmetric_glow || (line.right() - rect.right()).abs() > horizontal;
    let keep_top = hugs.top || rect.y - line.y > vertical_outward;
    let keep_bottom = hugs.bottom || line.bottom() - rect.bottom() > vertical_outward;
    let refined = Rect::from_edges(
        if keep_left { rect.x } else { line.x },
        if keep_top { rect.y } else { line.y },
        if keep_right {
            rect.right()
        } else {
            line.right()
        },
        if keep_bottom {
            rect.bottom()
        } else {
            line.bottom()
        },
    );

    if axis_coverage(rect.x, rect.right(), refined.x, refined.right()) < REFINE_MIN_AXIS_COVERAGE
        || axis_coverage(rect.y, rect.bottom(), refined.y, refined.bottom())
            < REFINE_MIN_AXIS_COVERAGE
    {
        return None;
    }
    if refined.height > rect.height * REFINE_MAX_HEIGHT_GROWTH + REFINE_MARGIN_MIN_PX {
        return None;
    }
    let total_weight: f32 = character_refs
        .iter()
        .map(|character| advance_weight(character))
        .sum();
    let mean_advance = refined.width / total_weight.max(1.0);
    let ratio = mean_advance / refined.height.max(1.0);
    if !(REFINE_MIN_ADVANCE_HEIGHT_RATIO..=REFINE_MAX_ADVANCE_HEIGHT_RATIO).contains(&ratio) {
        return None;
    }

    // The plausibility checks above validated the raw measurement; the box we
    // return additionally never leaves the detector's edge by more than the
    // antialiasing fringe (see REFINE_MAX_OUTWARD_PX for the evidence).
    let clamped = Rect::from_edges(
        refined.x.max(rect.x - REFINE_MAX_OUTWARD_PX),
        refined.y.max(rect.y - REFINE_MAX_OUTWARD_PX),
        refined.right().min(rect.right() + REFINE_MAX_OUTWARD_PX),
        refined.bottom().min(rect.bottom() + REFINE_MAX_OUTWARD_PX),
    );
    // Measured centres describe the horizontal partition; they are only
    // trustworthy when both horizontal extents were found in the pixels.
    // Whitespace disqualifies the partition outright: normalization inserts
    // spaces the game never rendered (camelCase UI labels split for lookup),
    // so the one-glyph-per-char assumption breaks and a valley found for a
    // synthetic space cuts through real ink. An empty vector tells the
    // caller to keep its CTC-derived centres. Kept centres are clamped into
    // the returned rect so the geometry stays self-consistent when an edge
    // was pulled back by the outward clamp.
    let has_whitespace = text.chars().any(char::is_whitespace);
    let char_centers = if keep_left || keep_right || has_whitespace {
        Vec::new()
    } else {
        measured
            .character_bounds
            .iter()
            .map(|bounds| (bounds.x + bounds.width / 2.0).clamp(clamped.x, clamped.right()))
            .collect()
    };
    Some(RefinedLineGeometry {
        rect: clamped,
        char_centers,
    })
}

/// Which sides of the measured line press against the search-region edge.
/// Edges clamped to the image boundary are exempt: text at the screen edge
/// really can end there.
#[derive(Debug, Clone, Copy)]
struct RegionEdgeHugs {
    left: bool,
    top: bool,
    right: bool,
    bottom: bool,
}

impl RegionEdgeHugs {
    fn all(self) -> bool {
        self.left && self.top && self.right && self.bottom
    }
}

fn region_edge_hugs(line: Rect, region: Rect, image_w: f32, image_h: f32) -> RegionEdgeHugs {
    let left = region.x.max(0.0);
    let top = region.y.max(0.0);
    let right = region.right().min(image_w);
    let bottom = region.bottom().min(image_h);
    RegionEdgeHugs {
        left: line.x <= left + REFINE_REGION_EDGE_EPS && left > 0.0,
        top: line.y <= top + REFINE_REGION_EDGE_EPS && top > 0.0,
        right: line.right() >= right - REFINE_REGION_EDGE_EPS && right < image_w,
        bottom: line.bottom() >= bottom - REFINE_REGION_EDGE_EPS && bottom < image_h,
    }
}

/// Fraction of the `[a0, a1]` interval covered by `[b0, b1]`.
fn axis_coverage(a0: f32, a1: f32, b0: f32, b1: f32) -> f32 {
    let overlap = (a1.min(b1) - a0.max(b0)).max(0.0);
    overlap / (a1 - a0).max(1.0)
}

#[derive(Debug, Clone, PartialEq)]
pub struct MeasuredCharacterGeometry {
    pub line_bounds: Rect,
    pub character_bounds: Vec<Rect>,
    pub confidence: f32,
    pub row_concentration: f32,
    pub boundary_contrast: f32,
}

/// Measure typographic character cells from the original frame pixels.
///
/// The existing line rectangle is a search region, not a source of character
/// positions. Boundaries are placed in low-energy valleys between glyphs and
/// the vertical extent is reduced to the dense glyph band. Low-confidence
/// measurements return `None` so bulk re-annotation cannot silently replace
/// manual ground truth with guesses.
pub fn measure_character_geometry(
    image: &RgbImage,
    detection: &ExpectedDetection,
) -> Option<MeasuredCharacterGeometry> {
    let characters = detection
        .characters
        .iter()
        .map(|character| character.text.as_str())
        .collect::<Vec<_>>();
    measure_text_geometry(image, detection.bounds, &characters)
}

/// Measure character cells for human-authored text inside a drawn line region.
pub fn measure_text_geometry(
    image: &RgbImage,
    line_region: Rect,
    characters: &[&str],
) -> Option<MeasuredCharacterGeometry> {
    measure_text_geometry_impl(image, line_region, characters, None)
}

/// Shared measurement. `core` is an optional frame-space rectangle the caller
/// trusts to sit on the text (the detector box during runtime refinement).
/// When present, the row/column energy thresholds are derived from the core's
/// statistics instead of the whole region's. An annotator-drawn label region
/// is itself tight around the text, so its 35th-percentile baseline sits on
/// glyph energy and excludes glow and antialiasing halo; a runtime search
/// region padded with background would dilute that baseline toward zero and
/// admit the halo. With core statistics the two measure with the same bar,
/// and when the core equals the region the behavior is identical.
/// (Padding the stats span by an annotator-like margin was tried to close
/// the residual ~2 px label-vs-runtime edge gap on soft-rendered text; the
/// full corpus scored it as a wash and slightly worse where it was supposed
/// to help, so the bare core span stands.)
fn measure_text_geometry_impl(
    image: &RgbImage,
    line_region: Rect,
    characters: &[&str],
    core: Option<Rect>,
) -> Option<MeasuredCharacterGeometry> {
    let count = characters.len();
    if count == 0 {
        return None;
    }

    let region = PixelRegion::from_rect(line_region, image.width(), image.height())?;
    if region.width() < MIN_LINE_SIDE || region.height() < MIN_LINE_SIDE {
        return None;
    }

    let energy = GradientEnergy::measure(image, region);
    let core_rows = core.map(|core| region.relative_row_range(core));
    let core_columns = core.map(|core| region.relative_column_range(core));
    let (measured_top, measured_bottom, row_concentration) =
        energy.glyph_rows(core_rows.as_ref())?;
    let minimum_height =
        ((region.height() as f32 * 0.60).ceil() as usize).min(region.height() as usize);
    let (top, bottom) = expand_band(
        measured_top,
        measured_bottom,
        minimum_height,
        region.height() as usize,
    );
    let columns = energy.column_energy(top, bottom);
    let weights = characters
        .iter()
        .map(|character| advance_weight(character))
        .collect::<Vec<_>>();

    let (left, right) = active_extent(&columns, core_columns.as_ref())?;
    let (left, right) = match core_columns {
        Some(core) => clamp_extent_to_connected(&columns, left, right, &core),
        None => (left, right),
    };
    if right <= left || right - left < count {
        return None;
    }
    let (boundaries, boundary_contrast) = partition_columns(&columns, left, right, &weights)?;
    let character_bounds = boundaries
        .windows(2)
        .map(|pair| {
            let (character_top, character_bottom) = energy
                .character_rows(pair[0], pair[1], top, bottom)
                .unwrap_or((top, bottom));
            Rect::new(
                region.left as f32 + pair[0] as f32,
                region.top as f32 + character_top as f32,
                (pair[1] - pair[0]) as f32,
                (character_bottom - character_top) as f32,
            )
        })
        .collect::<Vec<_>>();
    let first = character_bounds.first()?;
    let last = character_bounds.last()?;
    let line_top = character_bounds
        .iter()
        .map(|bounds| bounds.y)
        .fold(f32::INFINITY, f32::min);
    let line_bottom = character_bounds
        .iter()
        .map(|bounds| bounds.bottom())
        .fold(f32::NEG_INFINITY, f32::max);
    let line_bounds = Rect::new(
        first.x,
        line_top,
        last.right() - first.x,
        line_bottom - line_top,
    );
    let confidence = (row_concentration * 0.45 + boundary_contrast * 0.55).clamp(0.0, 1.0);
    (confidence >= MIN_CONFIDENCE).then_some(MeasuredCharacterGeometry {
        line_bounds,
        character_bounds,
        confidence,
        row_concentration,
        boundary_contrast,
    })
}

#[derive(Debug, Clone, Copy)]
struct PixelRegion {
    left: u32,
    top: u32,
    right: u32,
    bottom: u32,
}

impl PixelRegion {
    fn from_rect(rect: Rect, image_width: u32, image_height: u32) -> Option<Self> {
        let left = rect.x.floor().max(0.0) as u32;
        let top = rect.y.floor().max(0.0) as u32;
        let right = rect.right().ceil().clamp(0.0, image_width as f32) as u32;
        let bottom = rect.bottom().ceil().clamp(0.0, image_height as f32) as u32;
        (right > left && bottom > top).then_some(Self {
            left,
            top,
            right,
            bottom,
        })
    }

    fn width(self) -> u32 {
        self.right - self.left
    }

    fn height(self) -> u32 {
        self.bottom - self.top
    }

    /// Row indices of `rect`'s vertical span, relative to this region and
    /// clamped to it.
    fn relative_row_range(self, rect: Rect) -> std::ops::Range<usize> {
        let top = (rect.y - self.top as f32).max(0.0) as usize;
        let bottom = ((rect.bottom() - self.top as f32).ceil().max(0.0) as usize)
            .min(self.height() as usize);
        top.min(bottom)..bottom
    }

    /// Column indices of `rect`'s horizontal span, relative to this region and
    /// clamped to it.
    fn relative_column_range(self, rect: Rect) -> std::ops::Range<usize> {
        let left = (rect.x - self.left as f32).max(0.0) as usize;
        let right =
            ((rect.right() - self.left as f32).ceil().max(0.0) as usize).min(self.width() as usize);
        left.min(right)..right
    }
}

struct GradientEnergy {
    width: usize,
    height: usize,
    pixels: Vec<f32>,
}

impl GradientEnergy {
    fn measure(image: &RgbImage, region: PixelRegion) -> Self {
        let width = region.width() as usize;
        let height = region.height() as usize;
        let mut luma = vec![0.0; width * height];
        for y in 0..height {
            for x in 0..width {
                luma[y * width + x] =
                    pixel_luma(image.get_pixel(region.left + x as u32, region.top + y as u32));
            }
        }

        let mut pixels = vec![0.0; width * height];
        if width >= 3 && height >= 3 {
            for y in 1..height - 1 {
                for x in 1..width - 1 {
                    let horizontal = (luma[y * width + x + 1] - luma[y * width + x - 1]).abs();
                    let vertical = (luma[(y + 1) * width + x] - luma[(y - 1) * width + x]).abs();
                    pixels[y * width + x] = horizontal + vertical;
                }
            }
        }
        Self {
            width,
            height,
            pixels,
        }
    }

    fn glyph_rows(
        &self,
        stats_rows: Option<&std::ops::Range<usize>>,
    ) -> Option<(usize, usize, f32)> {
        let rows = (0..self.height)
            .map(|y| {
                self.pixels[y * self.width..(y + 1) * self.width]
                    .iter()
                    .sum()
            })
            .collect::<Vec<f32>>();
        let smoothed = smooth(&rows, 1);
        let stats = stats_slice(&smoothed, stats_rows);
        let peak = stats.iter().copied().fold(0.0_f32, f32::max);
        if peak <= ENERGY_FLOOR {
            return None;
        }
        let baseline = percentile(stats, 0.35);
        let threshold = baseline + (peak - baseline) * 0.18;
        let active = smoothed
            .iter()
            .enumerate()
            .filter_map(|(index, value)| (*value >= threshold).then_some(index))
            .collect::<Vec<_>>();
        let top = active.first().copied()?.saturating_sub(1);
        let bottom = (active.last().copied()? + 2).min(self.height);
        if bottom <= top {
            return None;
        }
        let total: f32 = smoothed.iter().sum();
        let inside: f32 = smoothed[top..bottom].iter().sum();
        let concentration = if total > 0.0 { inside / total } else { 0.0 };
        Some((top, bottom, concentration.clamp(0.0, 1.0)))
    }

    fn column_energy(&self, top: usize, bottom: usize) -> Vec<f32> {
        let columns = (0..self.width)
            .map(|x| (top..bottom).map(|y| self.pixels[y * self.width + x]).sum())
            .collect::<Vec<_>>();
        smooth(&columns, 1)
    }

    fn character_rows(
        &self,
        left: usize,
        right: usize,
        line_top: usize,
        line_bottom: usize,
    ) -> Option<(usize, usize)> {
        let rows = (line_top..line_bottom)
            .map(|y| {
                self.pixels[y * self.width + left..y * self.width + right]
                    .iter()
                    .sum()
            })
            .collect::<Vec<f32>>();
        let smoothed = smooth(&rows, 1);
        let peak = smoothed.iter().copied().fold(0.0_f32, f32::max);
        if peak <= ENERGY_FLOOR {
            return None;
        }
        let baseline = percentile(&smoothed, 0.25);
        let threshold = baseline + (peak - baseline) * 0.15;
        let first = smoothed.iter().position(|value| *value >= threshold)?;
        let last = smoothed.iter().rposition(|value| *value >= threshold)?;
        let measured_top = line_top + first.saturating_sub(1);
        let measured_bottom = line_top + (last + 2).min(rows.len());
        let minimum_height = ((line_bottom - line_top) as f32 * 0.50).ceil() as usize;
        let (relative_top, relative_bottom) = expand_band(
            measured_top - line_top,
            measured_bottom - line_top,
            minimum_height,
            line_bottom - line_top,
        );
        Some((line_top + relative_top, line_top + relative_bottom))
    }
}

fn pixel_luma(pixel: &Rgb<u8>) -> f32 {
    0.299 * f32::from(pixel[0]) + 0.587 * f32::from(pixel[1]) + 0.114 * f32::from(pixel[2])
}

fn smooth(values: &[f32], radius: usize) -> Vec<f32> {
    (0..values.len())
        .map(|index| {
            let start = index.saturating_sub(radius);
            let end = (index + radius + 1).min(values.len());
            values[start..end].iter().sum::<f32>() / (end - start) as f32
        })
        .collect()
}

fn expand_band(top: usize, bottom: usize, minimum_height: usize, limit: usize) -> (usize, usize) {
    if bottom.saturating_sub(top) >= minimum_height {
        return (top, bottom);
    }
    let center = (top + bottom) / 2;
    let mut expanded_top = center.saturating_sub(minimum_height / 2);
    let mut expanded_bottom = (expanded_top + minimum_height).min(limit);
    expanded_top = expanded_bottom.saturating_sub(minimum_height);
    expanded_bottom = (expanded_top + minimum_height).min(limit);
    (expanded_top, expanded_bottom)
}

fn percentile(values: &[f32], quantile: f32) -> f32 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f32::total_cmp);
    let index = ((sorted.len().saturating_sub(1)) as f32 * quantile)
        .round()
        .clamp(0.0, sorted.len().saturating_sub(1) as f32) as usize;
    sorted.get(index).copied().unwrap_or_default()
}

fn active_extent(
    columns: &[f32],
    stats_columns: Option<&std::ops::Range<usize>>,
) -> Option<(usize, usize)> {
    let stats = stats_slice(columns, stats_columns);
    let peak = stats.iter().copied().fold(0.0_f32, f32::max);
    if peak <= ENERGY_FLOOR {
        return None;
    }
    let baseline = percentile(stats, 0.35);
    let threshold = baseline + (peak - baseline) * 0.12;
    let left = columns
        .iter()
        .position(|value| *value >= threshold)?
        .saturating_sub(1);
    let right = (columns.iter().rposition(|value| *value >= threshold)? + 2).min(columns.len());
    (right > left).then_some((left, right))
}

/// The sub-slice threshold statistics are computed over: the trusted core's
/// span when one is provided and non-empty, otherwise the full profile.
fn stats_slice<'a>(values: &'a [f32], range: Option<&std::ops::Range<usize>>) -> &'a [f32] {
    match range {
        Some(range) if !range.is_empty() && range.end <= values.len() => &values[range.clone()],
        _ => values,
    }
}

/// Maximum run of below-threshold columns the extent may cross when growing
/// beyond the trusted core. Glyph-internal valleys are this narrow; the
/// padding between text and surrounding UI chrome (button frames, badge
/// borders) is wider, so a clipped edge glyph stays reachable while chrome
/// behind a clear gap does not.
const CONNECTED_EXTENT_MAX_GAP: usize = 3;

/// Clamp `[left, right)` so any part outside the core's column span is
/// reachable from the core through above-threshold columns with gaps of at
/// most [`CONNECTED_EXTENT_MAX_GAP`]. The extent inside the core is never
/// reduced. `active_extent` alone takes the first/last hot column anywhere in
/// the search region, which lets a detached decoration extend the extent
/// across dead space; an annotator-drawn label region has no such neighbors,
/// so this clamp applies only to core-guided (runtime) measurement.
fn clamp_extent_to_connected(
    columns: &[f32],
    left: usize,
    right: usize,
    core: &std::ops::Range<usize>,
) -> (usize, usize) {
    if core.is_empty() || core.end > columns.len() {
        return (left, right);
    }
    let stats = &columns[core.clone()];
    let peak = stats.iter().copied().fold(0.0_f32, f32::max);
    let baseline = percentile(stats, 0.35);
    let threshold = baseline + (peak - baseline) * 0.12;

    let mut lo = core.start.min(right);
    let mut gap = 0usize;
    let mut last_hot = lo;
    while lo > left {
        let column = lo - 1;
        if columns[column] >= threshold {
            last_hot = column;
            gap = 0;
        } else {
            gap += 1;
            if gap > CONNECTED_EXTENT_MAX_GAP {
                break;
            }
        }
        lo = column;
    }

    let mut hi = core.end.max(left).min(columns.len());
    let mut last_hot_right = hi;
    gap = 0;
    while hi < right {
        if columns[hi] >= threshold {
            last_hot_right = hi + 1;
            gap = 0;
        } else {
            gap += 1;
            if gap > CONNECTED_EXTENT_MAX_GAP {
                break;
            }
        }
        hi += 1;
    }

    let clamped_left = left.max(last_hot.saturating_sub(1)).min(core.start);
    let clamped_right = right
        .min(last_hot_right + 2)
        .max(core.end.min(columns.len()));
    (clamped_left, clamped_right.min(columns.len()))
}

fn advance_weight(text: &str) -> f32 {
    let Some(ch) = text.chars().next() else {
        return 1.0;
    };
    if ch.is_ascii_alphanumeric() {
        0.58
    } else if ch.is_ascii_punctuation() || ch == ' ' {
        0.45
    } else {
        1.0
    }
}

fn partition_columns(
    energy: &[f32],
    left: usize,
    right: usize,
    weights: &[f32],
) -> Option<(Vec<usize>, f32)> {
    if weights.is_empty() || right <= left {
        return None;
    }
    if weights.len() == 1 {
        return Some((vec![left, right], 1.0));
    }

    let total_weight: f32 = weights.iter().sum();
    let span = (right - left) as f32;
    let mean_cell = span / total_weight.max(1.0);
    // Use valleys for local alignment without letting a strong background edge
    // collapse a pale glyph into its neighbor. CJK ink widths vary much more
    // than their typographic advances.
    let search_radius = (mean_cell * 0.24).max(2.0);
    let local_peak_radius = (mean_cell * 0.35).max(2.0) as usize;
    let mut boundaries = Vec::with_capacity(weights.len() + 1);
    boundaries.push(left);
    let mut cumulative = 0.0;
    let mut contrasts = Vec::with_capacity(weights.len() - 1);

    for (weight_index, weight) in weights[..weights.len() - 1].iter().enumerate() {
        cumulative += *weight;
        let expected = left as f32 + span * cumulative / total_weight;
        let minimum_advance = (mean_cell * *weight * 0.58).max(1.0).floor() as usize;
        let remaining_minimum = weights[weight_index + 1..]
            .iter()
            .map(|remaining_weight| (mean_cell * *remaining_weight * 0.58).max(1.0))
            .sum::<f32>()
            .ceil() as usize;
        let minimum = ((expected - search_radius).floor() as isize)
            .max(boundaries.last().copied()? as isize + minimum_advance as isize)
            .max(left as isize + 1) as usize;
        let maximum = ((expected + search_radius).ceil() as usize)
            .min(right.saturating_sub(remaining_minimum));
        if maximum < minimum {
            return None;
        }
        let scale = percentile(&energy[left..right], 0.85).max(ENERGY_FLOOR);
        let best = (minimum..=maximum).min_by(|a, b| {
            boundary_cost(energy, *a, expected, search_radius, scale).total_cmp(&boundary_cost(
                energy,
                *b,
                expected,
                search_radius,
                scale,
            ))
        })?;
        let neighborhood_start = best.saturating_sub(local_peak_radius).max(left);
        let neighborhood_end = (best + local_peak_radius + 1).min(right);
        let local_peak = energy[neighborhood_start..neighborhood_end]
            .iter()
            .copied()
            .fold(0.0_f32, f32::max)
            .max(ENERGY_FLOOR);
        contrasts.push((1.0 - energy[best] / local_peak).clamp(0.0, 1.0));
        boundaries.push(best);
    }
    boundaries.push(right);
    if boundaries.windows(2).any(|pair| pair[1] <= pair[0]) {
        return None;
    }
    let contrast = contrasts.iter().sum::<f32>() / contrasts.len().max(1) as f32;
    Some((boundaries, contrast))
}

fn boundary_cost(energy: &[f32], position: usize, expected: f32, radius: f32, scale: f32) -> f32 {
    let visual = energy[position] / scale;
    let displacement = ((position as f32 - expected) / radius.max(1.0)).abs();
    visual + displacement * displacement * 0.45
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        eval::{ExpectedCharacter, ExpectedDetection},
        geometry::Rect,
    };

    #[test]
    fn measures_variable_width_character_cells_from_pixel_valleys() {
        let mut image = RgbImage::from_pixel(120, 50, Rgb([20, 20, 20]));
        for (left, right) in [(12, 31), (40, 50), (62, 91)] {
            for y in 12..39 {
                for x in left..right {
                    if x == left || x + 1 == right || y == 12 || y == 38 {
                        image.put_pixel(x, y, Rgb([235, 235, 235]));
                    }
                }
            }
        }
        let detection = fixture_detection(Rect::new(5.0, 5.0, 100.0, 40.0), "母は寒");

        let measured =
            measure_character_geometry(&image, &detection).expect("clear glyph geometry");

        assert_eq!(measured.character_bounds.len(), 3);
        assert!(measured.line_bounds.y >= 9.0);
        assert!(measured.line_bounds.bottom() <= 42.0);
        assert!(measured.character_bounds[0].right() < 45.0);
        assert!(measured.character_bounds[1].x > 30.0);
        assert!(measured.character_bounds[1].right() < 70.0);
        assert!(measured.character_bounds[2].x > 55.0);
        assert!(
            measured
                .character_bounds
                .iter()
                .all(|bounds| bounds.width >= 20.0),
            "a background valley must not collapse a glyph cell"
        );
    }

    #[test]
    fn refuses_flat_regions_without_pixel_evidence() {
        let image = RgbImage::from_pixel(100, 40, Rgb([80, 80, 80]));
        let detection = fixture_detection(Rect::new(0.0, 0.0, 100.0, 40.0), "文字");

        assert!(measure_character_geometry(&image, &detection).is_none());
    }

    #[test]
    fn preserves_typographic_height_when_only_one_antialiased_edge_is_strong() {
        assert_eq!(expand_band(8, 11, 12, 20), (3, 15));
        assert_eq!(expand_band(0, 3, 12, 20), (0, 12));
        assert_eq!(expand_band(17, 20, 12, 20), (8, 20));
    }

    #[test]
    fn measures_low_punctuation_independently_from_full_height_glyphs() {
        let mut image = RgbImage::from_pixel(80, 50, Rgb([20, 20, 20]));
        for y in 8..38 {
            image.put_pixel(12, y, Rgb([235, 235, 235]));
            image.put_pixel(31, y, Rgb([235, 235, 235]));
        }
        for y in 28..39 {
            image.put_pixel(48, y, Rgb([235, 235, 235]));
            image.put_pixel(61, y, Rgb([235, 235, 235]));
        }
        let detection = fixture_detection(Rect::new(5.0, 5.0, 65.0, 40.0), "今、");

        let measured =
            measure_character_geometry(&image, &detection).expect("clear glyph geometry");

        assert!(measured.character_bounds[1].y > measured.character_bounds[0].y);
        assert!(
            measured.character_bounds[1].height < measured.character_bounds[0].height,
            "punctuation should not inherit the full-height glyph box"
        );
    }

    /// Draw a row of hollow box "glyphs" and return their exact ink extents.
    fn draw_glyph_row(image: &mut RgbImage, spans: &[(u32, u32)], top: u32, bottom: u32) -> Rect {
        for &(left, right) in spans {
            for y in top..bottom {
                for x in left..right {
                    if x == left || x + 1 == right || y == top || y + 1 == bottom {
                        image.put_pixel(x, y, Rgb([235, 235, 235]));
                    }
                }
            }
        }
        let left = spans.first().map(|span| span.0).unwrap_or(0);
        let right = spans.last().map(|span| span.1).unwrap_or(0);
        Rect::new(
            left as f32,
            top as f32,
            (right - left) as f32,
            (bottom - top) as f32,
        )
    }

    #[test]
    fn refines_loose_detector_box_to_glyph_extent() {
        let mut image = RgbImage::from_pixel(160, 60, Rgb([20, 20, 20]));
        let ink = draw_glyph_row(&mut image, &[(20, 47), (52, 79), (84, 111)], 15, 45);
        // Detector box clips the first glyph's edge and misjudges the band,
        // the shape the DB mask typically produces.
        let detector = Rect::new(22.0, 14.0, 92.0, 32.0);

        let refined =
            refine_recognized_line(&image, detector, "母は寒").expect("clear glyphs must refine");

        assert!((refined.rect.x - ink.x).abs() <= 3.0, "left {refined:?}");
        assert!((refined.rect.right() - ink.right()).abs() <= 3.0, "right");
        assert!((refined.rect.y - ink.y).abs() <= 3.0, "top");
        assert!(
            (refined.rect.bottom() - ink.bottom()).abs() <= 3.0,
            "bottom"
        );
        assert_eq!(refined.char_centers.len(), 3);
        assert!(
            refined
                .char_centers
                .windows(2)
                .all(|pair| pair[0] < pair[1]),
            "centres must be ordered: {:?}",
            refined.char_centers
        );
    }

    #[test]
    fn clamps_outward_growth_to_the_antialiasing_fringe() {
        let mut image = RgbImage::from_pixel(160, 60, Rgb([20, 20, 20]));
        let ink = draw_glyph_row(&mut image, &[(20, 47), (52, 79), (84, 111)], 15, 45);
        // Detector box sits 4 px inside the true left edge: a real clipped
        // glyph, under the horizontal shift cap but beyond the outward trust
        // budget. The measurement finds the ink at x=20; the returned edge
        // stops at the clamp, not at the ink.
        let detector = Rect::new(24.0, 14.0, 90.0, 32.0);
        assert!(detector.x - ink.x > REFINE_MAX_OUTWARD_PX);

        let refined =
            refine_recognized_line(&image, detector, "母は寒").expect("clear glyphs must refine");

        assert!(
            (refined.rect.x - (detector.x - REFINE_MAX_OUTWARD_PX)).abs() < 0.5,
            "left edge should stop exactly at the outward clamp: {refined:?}"
        );
        assert!(refined.rect.right() <= detector.right() + REFINE_MAX_OUTWARD_PX + f32::EPSILON);
        assert!(refined.rect.y >= detector.y - REFINE_MAX_OUTWARD_PX - f32::EPSILON);
        assert!(refined.rect.bottom() <= detector.bottom() + REFINE_MAX_OUTWARD_PX + f32::EPSILON);
    }

    #[test]
    fn clamped_edges_keep_char_centers_inside_the_returned_rect() {
        let mut image = RgbImage::from_pixel(160, 60, Rgb([20, 20, 20]));
        let ink = draw_glyph_row(&mut image, &[(20, 47), (52, 79), (84, 111)], 15, 45);
        // Same shape as the clamp test: the measurement reaches the ink at
        // x=20 but the returned left edge stops at the clamp. The measured
        // first-glyph center must be pulled into the returned rect, not left
        // hanging outside it.
        let detector = Rect::new(24.0, 14.0, 90.0, 32.0);
        assert!(detector.x - ink.x > REFINE_MAX_OUTWARD_PX);

        let refined =
            refine_recognized_line(&image, detector, "母は寒").expect("clear glyphs must refine");

        assert_eq!(refined.char_centers.len(), 3);
        for center in &refined.char_centers {
            assert!(
                (refined.rect.x..=refined.rect.right()).contains(center),
                "center {center} escaped rect {:?}",
                refined.rect
            );
        }
    }

    #[test]
    fn whitespace_in_text_suppresses_center_replacement() {
        let mut image = RgbImage::from_pixel(160, 60, Rgb([20, 20, 20]));
        draw_glyph_row(&mut image, &[(20, 47), (52, 79), (84, 111)], 15, 45);
        let detector = Rect::new(22.0, 14.0, 92.0, 32.0);

        // Normalization inserts spaces the game never rendered, so a text
        // containing whitespace must still refine the box but hand center
        // placement back to the caller's CTC-derived values.
        let refined = refine_recognized_line(&image, detector, "母 寒")
            .expect("whitespace must not block box refinement");

        assert!(
            refined.char_centers.is_empty(),
            "centers must be suppressed for whitespace text: {refined:?}"
        );
    }

    #[test]
    fn keeps_detector_box_on_flat_pixels() {
        let image = RgbImage::from_pixel(120, 50, Rgb([80, 80, 80]));
        let detector = Rect::new(20.0, 10.0, 80.0, 30.0);

        assert!(refine_recognized_line(&image, detector, "文字").is_none());
    }

    #[test]
    fn rejects_measurement_that_spans_far_more_ink_than_the_text() {
        let mut image = RgbImage::from_pixel(300, 60, Rgb([20, 20, 20]));
        // Six glyphs of ink for a two-character read: the measured extent is
        // three times the plausible advance, so the guard must keep the
        // detector box instead of stretching it across the neighbor.
        draw_glyph_row(
            &mut image,
            &[
                (20, 47),
                (52, 79),
                (84, 111),
                (116, 143),
                (148, 175),
                (180, 207),
            ],
            15,
            45,
        );
        let detector = Rect::new(18.0, 14.0, 192.0, 32.0);

        assert!(refine_recognized_line(&image, detector, "母は").is_none());
    }

    #[test]
    fn rejects_textured_region_with_no_separable_extent() {
        let mut image = RgbImage::from_pixel(200, 80, Rgb([20, 20, 20]));
        // A 2 px checkerboard puts gradient energy everywhere, like game-world
        // texture behind a translucent panel: the measured "extent" runs to
        // the search-region edges and must not be trusted.
        for y in 0..80 {
            for x in 0..200 {
                if ((x / 2) + (y / 2)) % 2 == 0 {
                    image.put_pixel(x, y, Rgb([180, 180, 180]));
                }
            }
        }
        let detector = Rect::new(60.0, 25.0, 80.0, 30.0);

        assert!(refine_recognized_line(&image, detector, "設定").is_none());
    }

    #[test]
    fn rejects_band_that_barely_overlaps_the_detector_box() {
        let mut image = RgbImage::from_pixel(160, 80, Rgb([20, 20, 20]));
        draw_glyph_row(&mut image, &[(20, 47), (52, 79)], 10, 22);
        // Detector box sits mostly below the ink band; the measurement finds
        // the band above but must not claim it for this box.
        let detector = Rect::new(20.0, 20.0, 60.0, 24.0);

        assert!(refine_recognized_line(&image, detector, "母は").is_none());
    }

    fn fixture_detection(bounds: Rect, text: &str) -> ExpectedDetection {
        ExpectedDetection {
            id: "fixture".to_string(),
            bounds,
            character_geometry: crate::eval::CharacterGeometrySource::Manual,
            bounds_tolerance: None,
            text: text.to_string(),
            characters: text
                .chars()
                .map(|ch| ExpectedCharacter {
                    text: ch.to_string(),
                    bounds,
                    token_id: "token".to_string(),
                    notes: None,
                })
                .collect(),
            tokens: Vec::new(),
            notes: None,
        }
    }
}
