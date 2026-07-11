//! Pixel-derived geometry for manual eval characters.
//!
//! Eval text remains human-authored. This module only measures where that text
//! is rendered, using image gradients inside the annotator's line rectangle.

use image::{Rgb, RgbImage};

use crate::{eval::ExpectedDetection, geometry::Rect};

const MIN_LINE_SIDE: u32 = 3;
const ENERGY_FLOOR: f32 = 1.0;
const MIN_CONFIDENCE: f32 = 0.45;

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
    let count = characters.len();
    if count == 0 {
        return None;
    }

    let region = PixelRegion::from_rect(line_region, image.width(), image.height())?;
    if region.width() < MIN_LINE_SIDE || region.height() < MIN_LINE_SIDE {
        return None;
    }

    let energy = GradientEnergy::measure(image, region);
    let (measured_top, measured_bottom, row_concentration) = energy.glyph_rows()?;
    let minimum_height =
        ((region.height() as f32 * 0.60).ceil() as usize).min(region.height() as usize);
    let (top, bottom) = expand_band(
        measured_top,
        measured_bottom,
        minimum_height,
        region.height() as usize,
    );
    let columns = energy.column_energy(top, bottom);
    let (left, right) = active_extent(&columns)?;
    if right <= left || right - left < count {
        return None;
    }

    let weights = characters
        .iter()
        .map(|character| advance_weight(character))
        .collect::<Vec<_>>();
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

    fn glyph_rows(&self) -> Option<(usize, usize, f32)> {
        let rows = (0..self.height)
            .map(|y| {
                self.pixels[y * self.width..(y + 1) * self.width]
                    .iter()
                    .sum()
            })
            .collect::<Vec<f32>>();
        let smoothed = smooth(&rows, 1);
        let peak = smoothed.iter().copied().fold(0.0_f32, f32::max);
        if peak <= ENERGY_FLOOR {
            return None;
        }
        let baseline = percentile(&smoothed, 0.35);
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

fn active_extent(columns: &[f32]) -> Option<(usize, usize)> {
    let peak = columns.iter().copied().fold(0.0_f32, f32::max);
    if peak <= ENERGY_FLOOR {
        return None;
    }
    let baseline = percentile(columns, 0.35);
    let threshold = baseline + (peak - baseline) * 0.12;
    let left = columns
        .iter()
        .position(|value| *value >= threshold)?
        .saturating_sub(1);
    let right = (columns.iter().rposition(|value| *value >= threshold)? + 2).min(columns.len());
    (right > left).then_some((left, right))
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
