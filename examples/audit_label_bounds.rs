//! Audit eval label bounds against pixel evidence.
//!
//! For every unmatched expected detection in a corpus report directory, look
//! for an unexpected actual line with exactly the same recognized text in the
//! same neighborhood — the signature of a label whose drawn bounds disagree
//! with where the glyphs actually are. For each candidate pair, measure the
//! glyph band from the image (horizontal-gradient row energy) and report
//! whether the pixel evidence proves the label bounds wrong, along with a
//! proposed corrected rectangle.
//!
//! This tool only reports; it never edits labels. Usage:
//!   cargo run --release --example audit_label_bounds -- <report_dir> <eval_root> <out_json>

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
struct Report {
    image: String,
    eval: String,
    detections: Vec<Detection>,
    #[serde(default)]
    unexpected_actual: Vec<ActualLine>,
}

#[derive(Debug, Deserialize)]
struct Detection {
    id: String,
    expected_bounds: RectJson,
    expected_text: String,
    actual: Option<ActualLine>,
    #[serde(default)]
    detection_score: f32,
}

#[derive(Debug, Deserialize)]
struct ActualLine {
    text: String,
    text_box: TextBoxJson,
}

#[derive(Debug, Deserialize)]
struct TextBoxJson {
    rect: RectJson,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct RectJson {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

#[derive(Debug, Serialize)]
struct Finding {
    eval: String,
    detection_id: String,
    text: String,
    label_bounds: RectJson,
    ocr_bounds: RectJson,
    glyph_band_y: (u32, u32),
    label_band_overlap: f32,
    ocr_band_overlap: f32,
    verdict: &'static str,
    proposed_bounds: Option<RectJson>,
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let report_dir = args
        .first()
        .map(String::as_str)
        .unwrap_or("target/eval/corpus-w2");
    let out_path = args
        .get(2)
        .map(String::as_str)
        .unwrap_or("target/eval/label-bounds-audit.json");

    let mut findings: Vec<Finding> = Vec::new();
    for entry in fs::read_dir(report_dir).context("read report dir")? {
        let path = entry?.path();
        if path.extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        let report: Report = serde_json::from_str(&fs::read_to_string(&path)?)
            .with_context(|| format!("parse {}", path.display()))?;

        let mut image: Option<image::RgbImage> = None;
        for det in &report.detections {
            let exp = det.expected_bounds;
            // Two label-error signatures: an unmatched expected detection
            // whose exact text shows up as an unexpected actual nearby, and a
            // matched detection with identical text whose bounds credit is
            // still poor — both mean label geometry disagrees with the glyphs.
            let ua = match &det.actual {
                Some(actual) => {
                    if actual.text != det.expected_text || det.detection_score >= 0.70 {
                        continue;
                    }
                    actual
                }
                None => {
                    let Some(ua) = report.unexpected_actual.iter().find(|ua| {
                        ua.text == det.expected_text
                            && (ua.text_box.rect.x - exp.x).abs() < exp.height * 3.0
                            && (ua.text_box.rect.y - exp.y).abs() < exp.height * 3.0
                    }) else {
                        continue;
                    };
                    ua
                }
            };

            if image.is_none() {
                image = Some(
                    image::open(Path::new(&report.image))
                        .with_context(|| format!("open {}", report.image))?
                        .to_rgb8(),
                );
            }
            let frame = image.as_ref().expect("image loaded");
            let ocr = ua.text_box.rect;
            let Some((band_top, band_bottom)) = glyph_row_band(frame, exp, ocr) else {
                continue;
            };

            // Fraction of the glyph band the label misses, and how much of the
            // OCR rect lies on glyph rows. A label is provably misplaced when a
            // large share of the real glyph rows fall outside it while the OCR
            // rect sits fully on them.
            let band_h = (band_bottom - band_top) as f32;
            let label_hit = band_overlap(band_top, band_bottom, exp.y, exp.y + exp.height);
            let band_outside_label = 1.0 - label_hit;
            let ocr_rows_on_band = rect_coverage_by_band(band_top, band_bottom, ocr.y, ocr.height);
            let label_wrong = band_outside_label >= 0.40 && ocr_rows_on_band >= 0.90;
            let proposed = if label_wrong {
                glyph_column_extent(
                    image.as_ref().expect("image loaded"),
                    band_top,
                    band_bottom,
                    exp,
                    ocr,
                )
                .map(|(x_left, x_right)| RectJson {
                    x: x_left as f32,
                    y: band_top as f32,
                    width: (x_right - x_left) as f32,
                    height: band_h,
                })
            } else {
                None
            };
            let label_overlap = label_hit;
            let ocr_overlap = ocr_rows_on_band;
            findings.push(Finding {
                eval: report.eval.clone(),
                detection_id: det.id.clone(),
                text: det.expected_text.clone(),
                label_bounds: exp,
                ocr_bounds: ocr,
                glyph_band_y: (band_top, band_bottom),
                label_band_overlap: label_overlap,
                ocr_band_overlap: ocr_overlap,
                verdict: if label_wrong {
                    "label-bounds-provably-wrong"
                } else {
                    "inconclusive"
                },
                proposed_bounds: proposed,
            });
        }
    }

    fs::create_dir_all(PathBuf::from(out_path).parent().unwrap_or(Path::new(".")))?;
    fs::write(out_path, serde_json::to_string_pretty(&findings)?)?;
    println!(
        "{} findings ({} provable) -> {}",
        findings.len(),
        findings
            .iter()
            .filter(|f| f.verdict == "label-bounds-provably-wrong")
            .count(),
        out_path
    );
    for finding in &findings {
        println!(
            "{} | {} | {:?} | label y {:.0}+{:.0} ocr y {:.0}+{:.0} band {}..{} | label_ov {:.2} ocr_ov {:.2}",
            finding.verdict,
            finding.detection_id,
            finding.text,
            finding.label_bounds.y,
            finding.label_bounds.height,
            finding.ocr_bounds.y,
            finding.ocr_bounds.height,
            finding.glyph_band_y.0,
            finding.glyph_band_y.1,
            finding.label_band_overlap,
            finding.ocr_band_overlap,
        );
    }
    Ok(())
}

/// Contiguous high-energy glyph row band covering the union of both rects,
/// from horizontal luminance gradients (text rows have dense vertical edges).
fn glyph_row_band(image: &image::RgbImage, a: RectJson, b: RectJson) -> Option<(u32, u32)> {
    let margin = a.height.max(b.height);
    let x0 = (a.x.min(b.x).max(0.0)) as u32;
    let x1 = ((a.x + a.width)
        .max(b.x + b.width)
        .min(image.width() as f32 - 1.0)) as u32;
    let y0 = ((a.y.min(b.y) - margin).max(0.0)) as u32;
    let y1 = ((a.y + a.height)
        .max(b.y + b.height)
        .min(image.height() as f32 - 1.0) as u32)
        .min(image.height() - 1);
    if x1 <= x0 + 4 || y1 <= y0 + 2 {
        return None;
    }

    let luma = |x: u32, y: u32| -> f32 {
        let p = image.get_pixel(x, y);
        0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32
    };
    let energies: Vec<f32> = (y0..=y1)
        .map(|y| (x0..x1).map(|x| (luma(x + 1, y) - luma(x, y)).abs()).sum())
        .collect();
    let peak = energies.iter().copied().fold(0.0f32, f32::max);
    if peak <= 1.0 {
        return None;
    }
    let threshold = peak * 0.25;
    let first = energies.iter().position(|&e| e >= threshold)?;
    let last = energies.iter().rposition(|&e| e >= threshold)?;
    Some((y0 + first as u32, y0 + last as u32 + 1))
}

fn band_overlap(band_top: u32, band_bottom: u32, top: f32, bottom: f32) -> f32 {
    let band_h = (band_bottom - band_top) as f32;
    if band_h <= 0.0 {
        return 0.0;
    }
    let overlap = (bottom.min(band_bottom as f32) - top.max(band_top as f32)).max(0.0);
    overlap / band_h
}

/// Fraction of a rect's rows that lie on the glyph band.
fn rect_coverage_by_band(band_top: u32, band_bottom: u32, top: f32, height: f32) -> f32 {
    if height <= 0.0 {
        return 0.0;
    }
    let overlap = ((top + height).min(band_bottom as f32) - top.max(band_top as f32)).max(0.0);
    overlap / height
}

/// Horizontal glyph extent within the band rows, from column gradient energy.
fn glyph_column_extent(
    image: &image::RgbImage,
    band_top: u32,
    band_bottom: u32,
    a: RectJson,
    b: RectJson,
) -> Option<(u32, u32)> {
    let margin = a.height.max(b.height);
    let x0 = ((a.x.min(b.x) - margin).max(0.0)) as u32;
    let x1 = ((a.x + a.width).max(b.x + b.width) + margin).min(image.width() as f32 - 2.0) as u32;
    let y1 = band_bottom.min(image.height() - 2);
    if x1 <= x0 + 4 || y1 <= band_top {
        return None;
    }
    let luma = |x: u32, y: u32| -> f32 {
        let p = image.get_pixel(x, y);
        0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32
    };
    let energies: Vec<f32> = (x0..=x1)
        .map(|x| {
            (band_top..y1)
                .map(|y| (luma(x, y + 1) - luma(x, y)).abs())
                .sum()
        })
        .collect();
    let peak = energies.iter().copied().fold(0.0f32, f32::max);
    if peak <= 1.0 {
        return None;
    }
    let threshold = peak * 0.20;
    let first = energies.iter().position(|&e| e >= threshold)?;
    let last = energies.iter().rposition(|&e| e >= threshold)?;
    Some((x0 + first as u32, x0 + last as u32 + 1))
}
