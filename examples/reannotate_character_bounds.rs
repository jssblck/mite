//! Audit or apply pixel-derived character geometry across eval bundles.

use std::{fs, path::PathBuf};

use anyhow::{Context, Result};
use clap::Parser;
use mite::{
    artifact,
    eval::{self, CharacterGeometrySource},
    eval_geometry::measure_character_geometry,
};
use serde::Serialize;

#[derive(Debug, Parser)]
struct Args {
    /// Corpus root containing capture folders with eval.json files.
    #[arg(long, default_value = "eval")]
    root: PathBuf,

    /// Structured audit report path.
    #[arg(long, default_value = "target/eval/character-geometry-audit.json")]
    out: PathBuf,

    /// Rewrite labels whose pixel measurement meets this confidence floor.
    #[arg(long)]
    apply: bool,

    /// Re-measure detections that already carry geometry provenance.
    #[arg(long)]
    force: bool,

    #[arg(long, default_value_t = 0.62)]
    min_confidence: f32,

    /// Optional directory for full-frame old/red and proposed/green overlays.
    #[arg(long)]
    preview_dir: Option<PathBuf>,
}

#[derive(Debug, Serialize)]
struct AuditReport {
    eval_files: usize,
    detections: usize,
    measured: usize,
    applied: usize,
    skipped: usize,
    findings: Vec<Finding>,
}

#[derive(Debug, Serialize)]
struct Finding {
    eval: String,
    detection_id: String,
    confidence: Option<f32>,
    row_concentration: Option<f32>,
    boundary_contrast: Option<f32>,
    old_bounds: mite::geometry::Rect,
    proposed_bounds: Option<mite::geometry::Rect>,
    old_character_bounds: Vec<mite::geometry::Rect>,
    proposed_character_bounds: Option<Vec<mite::geometry::Rect>>,
    applied: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let mut eval_paths = Vec::new();
    collect_eval_paths(&args.root, &mut eval_paths)?;
    eval_paths.sort();

    let results = eval_paths
        .iter()
        .map(|path| process_eval(path, &args))
        .collect::<Result<Vec<_>>>()?;
    let mut findings = Vec::new();
    let mut detections = 0;
    let mut measured = 0;
    let mut applied = 0;
    for result in results {
        detections += result.detections;
        measured += result.measured;
        applied += result.applied;
        findings.extend(result.findings);
    }
    let report = AuditReport {
        eval_files: eval_paths.len(),
        detections,
        measured,
        applied,
        skipped: detections - measured,
        findings,
    };
    artifact::write_json_pretty(&args.out, &report)?;
    println!(
        "{} eval files | {} detections | {} measured | {} applied | {} skipped -> {}",
        report.eval_files,
        report.detections,
        report.measured,
        report.applied,
        report.skipped,
        args.out.display()
    );
    Ok(())
}

struct EvalResult {
    detections: usize,
    measured: usize,
    applied: usize,
    findings: Vec<Finding>,
}

fn process_eval(path: &std::path::Path, args: &Args) -> Result<EvalResult> {
    let checked = eval::load_eval_spec(path)?;
    let mut spec = checked.into_inner();
    let image_name = spec.image.as_deref().unwrap_or("underlying.png");
    let image_path = path
        .parent()
        .context("eval path must have a parent directory")?
        .join(image_name);
    let image = image::open(&image_path)
        .with_context(|| format!("failed to open {}", image_path.display()))?
        .to_rgb8();
    let mut preview = args.preview_dir.as_ref().map(|_| image.clone());
    let mut measured_count = 0;
    let mut applied_count = 0;
    let mut findings = Vec::with_capacity(spec.detections.len());

    for detection in &mut spec.detections {
        if !args.force {
            if let Some(preview) = &mut preview {
                draw_rect(preview, detection.bounds, image::Rgb([88, 196, 143]));
                for character in &detection.characters {
                    draw_rect(preview, character.bounds, image::Rgb([70, 235, 130]));
                }
            }
            findings.push(Finding {
                eval: path.display().to_string(),
                detection_id: detection.id.clone(),
                confidence: None,
                row_concentration: None,
                boundary_contrast: None,
                old_bounds: detection.bounds,
                proposed_bounds: None,
                old_character_bounds: detection
                    .characters
                    .iter()
                    .map(|character| character.bounds)
                    .collect(),
                proposed_character_bounds: None,
                applied: false,
            });
            continue;
        }
        let old_bounds = detection.bounds;
        let old_character_bounds = detection
            .characters
            .iter()
            .map(|character| character.bounds)
            .collect::<Vec<_>>();
        let measured = measure_character_geometry(&image, detection);
        let should_apply = measured
            .as_ref()
            .is_some_and(|geometry| args.apply && geometry.confidence >= args.min_confidence);
        if let Some(geometry) = &measured {
            measured_count += 1;
            if let Some(preview) = &mut preview {
                draw_rect(preview, old_bounds, image::Rgb([239, 106, 106]));
                for bounds in &old_character_bounds {
                    draw_rect(preview, *bounds, image::Rgb([180, 70, 70]));
                }
                draw_rect(preview, geometry.line_bounds, image::Rgb([88, 196, 143]));
                for bounds in &geometry.character_bounds {
                    draw_rect(preview, *bounds, image::Rgb([70, 235, 130]));
                }
            }
            if should_apply {
                detection.bounds = geometry.line_bounds;
                detection.character_geometry = CharacterGeometrySource::PixelGradientV1;
                for (character, bounds) in detection
                    .characters
                    .iter_mut()
                    .zip(&geometry.character_bounds)
                {
                    character.bounds = *bounds;
                }
                applied_count += 1;
            }
        }
        findings.push(Finding {
            eval: path.display().to_string(),
            detection_id: detection.id.clone(),
            confidence: measured.as_ref().map(|geometry| geometry.confidence),
            row_concentration: measured.as_ref().map(|geometry| geometry.row_concentration),
            boundary_contrast: measured.as_ref().map(|geometry| geometry.boundary_contrast),
            old_bounds,
            proposed_bounds: measured.as_ref().map(|geometry| geometry.line_bounds),
            old_character_bounds,
            proposed_character_bounds: measured
                .as_ref()
                .map(|geometry| geometry.character_bounds.clone()),
            applied: should_apply,
        });
    }

    if let (Some(preview_dir), Some(preview)) = (&args.preview_dir, preview) {
        let relative = path.strip_prefix(&args.root).unwrap_or(path);
        let preview_path = preview_dir
            .join(relative)
            .with_file_name("geometry-preview.png");
        if let Some(parent) = preview_path.parent() {
            fs::create_dir_all(parent)?;
        }
        preview
            .save(&preview_path)
            .with_context(|| format!("failed to save {}", preview_path.display()))?;
    }

    if applied_count > 0 {
        let checked = eval::parse_eval_spec(spec)?;
        artifact::write_json_pretty(path, checked.get())?;
    }
    Ok(EvalResult {
        detections: findings.len(),
        measured: measured_count,
        applied: applied_count,
        findings,
    })
}

fn draw_rect(image: &mut image::RgbImage, rect: mite::geometry::Rect, color: image::Rgb<u8>) {
    let left = rect.x.floor().max(0.0) as u32;
    let top = rect.y.floor().max(0.0) as u32;
    let right = rect.right().ceil().clamp(0.0, image.width() as f32) as u32;
    let bottom = rect.bottom().ceil().clamp(0.0, image.height() as f32) as u32;
    if right <= left || bottom <= top {
        return;
    }
    for x in left..right {
        if top < image.height() {
            image.put_pixel(x, top, color);
        }
        if bottom > 0 && bottom - 1 < image.height() {
            image.put_pixel(x, bottom - 1, color);
        }
    }
    for y in top..bottom {
        if left < image.width() {
            image.put_pixel(left, y, color);
        }
        if right > 0 && right - 1 < image.width() {
            image.put_pixel(right - 1, y, color);
        }
    }
}

fn collect_eval_paths(root: &std::path::Path, paths: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(root).with_context(|| format!("failed to read {}", root.display()))? {
        let path = entry?.path();
        if path.is_dir() {
            collect_eval_paths(&path, paths)?;
        } else if path.file_name().is_some_and(|name| name == "eval.json") {
            paths.push(path);
        }
    }
    Ok(())
}
