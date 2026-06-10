//! Full-frame image eval scoring for real game captures.
//!
//! Eval labels are manual ground truth. Mite runs a fresh OCR + lookup pass on
//! the supplied image, then scores its detections, recognized characters, and
//! lookup/popup metadata against the provided `eval.json`.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::capture::{FrameSource, ImageFileCapture};
use crate::config::{AppConfig, PipelineConfig};
use crate::dictionary::{Dictionary, Token};
use crate::geometry::Rect;
use crate::hover::{
    FuriSegment, WordCategory, categorize, popup_content, token_at_char, token_spans,
    transitivity_hint,
};
use crate::ocr::{OcrEngine, TextBox, build_ocr_engine, filter_recognized_items};
use crate::text_blocks::{
    AnalyzedLine, LineToken, TextSpan, analyze_recognized_lines, sort_recognized_reading_order,
};

const DEFAULT_MAX_SENSES: usize = 3;
const DEFAULT_MAX_GLOSSES: usize = 4;
const AGGREGATE_DETECTION_WEIGHT: f32 = 0.35;
const AGGREGATE_CHARACTER_WEIGHT: f32 = 0.40;
const AGGREGATE_METADATA_WEIGHT: f32 = 0.25;
const PERFECT_SCORE_EPSILON: f32 = 0.0001;
const DEFAULT_BOUNDS_POSITION_TOLERANCE_PX: f32 = 4.0;
const DEFAULT_BOUNDS_POSITION_TOLERANCE_HEIGHT_RATIO: f32 = 0.20;
const DEFAULT_BOUNDS_SIZE_TOLERANCE_PX: f32 = 6.0;
const DEFAULT_BOUNDS_SIZE_TOLERANCE_HEIGHT_RATIO: f32 = 0.30;
const BOUNDS_ZERO_SCORE_MULTIPLIER: f32 = 3.0;
const TOLERANT_MATCH_MIN_COVERAGE: f32 = 0.35;

/// One image threaded through OCR and dictionary lookup.
#[derive(Debug, Clone, Serialize)]
pub struct OcrLookupResult {
    pub image: String,
    pub recognized_text: String,
    pub lines: Vec<LineLookup>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LineLookup {
    pub text_box: TextBox,
    pub text: String,
    pub confidence: f32,
    pub reused: bool,
    pub char_centers: Vec<f32>,
    pub block_id: usize,
    pub block_text: String,
    pub block_span: TextSpan,
    pub block_tokens: Vec<Token>,
    pub tokens: Vec<LineToken>,
}

impl OcrLookupResult {
    /// All resolved dictionary forms (known tokens), in order of appearance.
    pub fn resolved_forms(&self) -> Vec<String> {
        let mut seen_blocks = HashSet::new();
        self.lines
            .iter()
            .filter(|line| seen_blocks.insert(line.block_id))
            .flat_map(|line| line.block_tokens.iter())
            .filter(|token| token.is_known())
            .map(|token| token.dictionary_form.clone())
            .collect()
    }
}

/// Run OCR on an image, then segment + look up the recognized text.
pub fn ocr_lookup_image(
    engine: &mut dyn OcrEngine,
    pipeline: &PipelineConfig,
    dict: &Dictionary,
    image: &Path,
) -> Result<OcrLookupResult> {
    let mut frame_source = ImageFileCapture::new(image)?;
    let frame = frame_source.next_frame()?;

    let boxes = engine.detect(&frame, pipeline)?;
    let mut items = filter_recognized_items(engine.recognize(&frame, &boxes)?, pipeline);
    sort_recognized_reading_order(&mut items);

    let lines: Vec<LineLookup> = analyze_recognized_lines(dict, &items)
        .into_iter()
        .map(line_lookup_from_analyzed_line)
        .collect();

    let recognized_text = items
        .iter()
        .map(|item| item.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");

    Ok(OcrLookupResult {
        image: image.display().to_string(),
        recognized_text,
        lines,
    })
}

fn line_lookup_from_analyzed_line(line: AnalyzedLine) -> LineLookup {
    LineLookup {
        text_box: line.item.text_box,
        text: line.item.text,
        confidence: line.item.confidence,
        reused: line.item.reused,
        char_centers: line.item.char_centers,
        block_id: line.block_id,
        block_text: line.block_text,
        block_span: line.block_span,
        block_tokens: line.block_tokens,
        tokens: line.tokens,
    }
}

/// Inspect one image through OCR and dictionary lookup.
pub fn run_ocr_lookup(config: &AppConfig, image: &Path, lexicon: &Path) -> Result<OcrLookupResult> {
    let dict = Dictionary::load(lexicon)?;
    let mut engine = build_ocr_engine(&config.runtime, &config.models)?;
    ocr_lookup_image(&mut *engine, &config.pipeline, &dict, image)
}

#[derive(Debug, Clone, Copy)]
pub struct EvalOptions {
    pub min_iou: f32,
}

impl Default for EvalOptions {
    fn default() -> Self {
        Self { min_iou: 0.50 }
    }
}

#[derive(Debug, Clone)]
pub struct EvalBundle {
    pub image: PathBuf,
    pub labels: PathBuf,
}

#[derive(Debug, Clone)]
pub struct EvalCorpusOptions {
    pub min_iou: f32,
    pub out_dir: Option<PathBuf>,
    pub progress: bool,
}

impl Default for EvalCorpusOptions {
    fn default() -> Self {
        Self {
            min_iou: EvalOptions::default().min_iou,
            out_dir: None,
            progress: false,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct EvalCorpusReport {
    pub artifact_version: u32,
    pub root: String,
    pub eval_count: usize,
    pub passed_count: usize,
    pub failed_count: usize,
    pub aggregate_score: f32,
    pub detection_score: f32,
    pub character_score: f32,
    pub metadata_score: f32,
    pub entries: Vec<EvalCorpusEntry>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EvalCorpusEntry {
    pub image: String,
    pub eval: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub report: Option<String>,
    pub aggregate_score: f32,
    pub detection_score: f32,
    pub character_score: f32,
    pub metadata_score: f32,
    pub expected_detection_count: usize,
    pub actual_detection_count: usize,
    pub matched_detection_count: usize,
    pub ignored_actual_count: usize,
    pub unexpected_actual_count: usize,
    pub passed: bool,
}

#[derive(Debug, Default)]
struct CorpusScoreTotals {
    detection_credit: f32,
    detection_denominator: usize,
    character_errors: usize,
    character_denominator: usize,
    metadata_credit: f32,
    metadata_denominator: usize,
}

impl CorpusScoreTotals {
    fn add_report(&mut self, report: &EvalReport) {
        self.detection_credit += report
            .detections
            .iter()
            .map(|detection| detection.detection_score)
            .sum::<f32>();
        self.detection_denominator +=
            report.expected_detection_count + report.unexpected_actual_count;

        self.character_errors += report
            .detections
            .iter()
            .map(|detection| detection.char_edit_distance)
            .sum::<usize>()
            + report
                .unexpected_actual
                .iter()
                .map(|actual| actual.text.chars().count())
                .sum::<usize>();
        self.character_denominator += report
            .detections
            .iter()
            .map(|detection| {
                let expected_len = detection.expected_text.chars().count();
                let actual_len = detection
                    .actual
                    .as_ref()
                    .map(|actual| actual.text.chars().count())
                    .unwrap_or(0);
                expected_len.max(actual_len)
            })
            .sum::<usize>()
            + report
                .unexpected_actual
                .iter()
                .map(|actual| actual.text.chars().count())
                .sum::<usize>();

        self.metadata_credit += report
            .detections
            .iter()
            .flat_map(|detection| detection.token_scores.iter())
            .map(|token| token.metadata_score)
            .sum::<f32>();
        self.metadata_denominator += report
            .detections
            .iter()
            .map(|detection| detection.token_scores.len())
            .sum::<usize>()
            + report
                .unexpected_actual
                .iter()
                .map(|actual| actual.tokens.len().max(1))
                .sum::<usize>();
    }

    fn detection_score(&self) -> f32 {
        ratio_or_perfect(self.detection_credit, self.detection_denominator)
    }

    fn character_score(&self) -> f32 {
        if self.character_denominator == 0 {
            1.0
        } else {
            1.0 - (self.character_errors as f32 / self.character_denominator as f32)
        }
    }

    fn metadata_score(&self) -> f32 {
        ratio_or_perfect(self.metadata_credit, self.metadata_denominator)
    }

    fn aggregate_score(&self) -> f32 {
        self.detection_score() * AGGREGATE_DETECTION_WEIGHT
            + self.character_score() * AGGREGATE_CHARACTER_WEIGHT
            + self.metadata_score() * AGGREGATE_METADATA_WEIGHT
    }
}

fn ratio_or_perfect(numerator: f32, denominator: usize) -> f32 {
    if denominator == 0 {
        1.0
    } else {
        numerator / denominator as f32
    }
}

pub fn discover_eval_bundles(root: &Path) -> Result<Vec<EvalBundle>> {
    let mut labels = Vec::new();
    collect_eval_files(root, &mut labels)?;
    labels.sort();

    labels
        .into_iter()
        .map(|labels| {
            let image = labels
                .parent()
                .with_context(|| format!("eval path has no parent: {}", labels.display()))?
                .join("underlying.png");
            if !image.is_file() {
                bail!(
                    "eval labels {} have no sibling underlying.png",
                    labels.display()
                );
            }
            Ok(EvalBundle { image, labels })
        })
        .collect()
}

fn collect_eval_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("failed to read {}", dir.display()))? {
        let entry =
            entry.with_context(|| format!("failed to read entry under {}", dir.display()))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .with_context(|| format!("failed to inspect {}", path.display()))?;
        if file_type.is_dir() {
            collect_eval_files(&path, out)?;
        } else if file_type.is_file() && entry.file_name() == "eval.json" {
            out.push(path);
        }
    }
    Ok(())
}

pub fn run_eval_corpus(
    config: &AppConfig,
    root: &Path,
    lexicon: &Path,
    options: EvalCorpusOptions,
) -> Result<EvalCorpusReport> {
    let bundles = discover_eval_bundles(root)?;
    if bundles.is_empty() {
        bail!("no eval.json files found under {}", root.display());
    }

    let dict = Dictionary::load(lexicon)?;
    let mut engine = build_ocr_engine(&config.runtime, &config.models)?;
    let mut entries = Vec::with_capacity(bundles.len());
    let mut totals = CorpusScoreTotals::default();
    let total = bundles.len();

    for (index, bundle) in bundles.into_iter().enumerate() {
        if options.progress {
            eprintln!(
                "eval corpus {}/{}: {}",
                index + 1,
                total,
                display_relative(root, &bundle.labels)
            );
        }

        let spec = load_eval_spec(&bundle.labels)?;
        validate_eval_spec(&spec)?;
        let result = ocr_lookup_image(&mut *engine, &config.pipeline, &dict, &bundle.image)?;
        let report = score_ocr_lookup(
            &bundle.image,
            &bundle.labels,
            &spec,
            &result,
            options.min_iou,
        );
        totals.add_report(&report);

        let report_path = options
            .out_dir
            .as_ref()
            .map(|out_dir| corpus_entry_report_path(out_dir, root, &bundle.labels))
            .transpose()?;
        if let Some(path) = &report_path {
            crate::artifact::write_json_pretty(path, &report)?;
        }

        entries.push(EvalCorpusEntry {
            image: bundle.image.display().to_string(),
            eval: bundle.labels.display().to_string(),
            report: report_path.as_ref().map(|path| path.display().to_string()),
            aggregate_score: report.aggregate_score,
            detection_score: report.detection_score,
            character_score: report.character_score,
            metadata_score: report.metadata_score,
            expected_detection_count: report.expected_detection_count,
            actual_detection_count: report.actual_detection_count,
            matched_detection_count: report.matched_detection_count,
            ignored_actual_count: report.ignored_actual_count,
            unexpected_actual_count: report.unexpected_actual_count,
            passed: report.passed,
        });
    }

    let passed_count = entries.iter().filter(|entry| entry.passed).count();
    let failed_count = entries.len() - passed_count;
    Ok(EvalCorpusReport {
        artifact_version: crate::artifact::ARTIFACT_VERSION,
        root: root.display().to_string(),
        eval_count: entries.len(),
        passed_count,
        failed_count,
        aggregate_score: totals.aggregate_score(),
        detection_score: totals.detection_score(),
        character_score: totals.character_score(),
        metadata_score: totals.metadata_score(),
        entries,
    })
}

fn corpus_entry_report_path(out_dir: &Path, root: &Path, labels: &Path) -> Result<PathBuf> {
    let bundle_dir = labels
        .parent()
        .with_context(|| format!("eval path has no parent: {}", labels.display()))?;
    let relative = bundle_dir.strip_prefix(root).unwrap_or(bundle_dir);
    let mut stem = relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("__");
    if stem.is_empty() {
        stem.push_str("eval");
    }
    Ok(out_dir.join(format!("{stem}.json")))
}

fn display_relative<'a>(root: &Path, path: &'a Path) -> std::borrow::Cow<'a, str> {
    path.strip_prefix(root).unwrap_or(path).to_string_lossy()
}

pub fn render_eval_corpus_report(report: &EvalCorpusReport, worst_limit: usize) {
    println!(
        "eval corpus: aggregate {:.2}% | detection {:.2}% | characters {:.2}% | metadata {:.2}% | passed {}/{}",
        report.aggregate_score * 100.0,
        report.detection_score * 100.0,
        report.character_score * 100.0,
        report.metadata_score * 100.0,
        report.passed_count,
        report.eval_count
    );

    let mut worst = report.entries.iter().collect::<Vec<_>>();
    worst.sort_by(|left, right| left.aggregate_score.total_cmp(&right.aggregate_score));
    for entry in worst
        .into_iter()
        .filter(|entry| !entry.passed)
        .take(worst_limit)
    {
        println!(
            "  {:.2}% | det {:.1}% char {:.1}% meta {:.1}% | matched {}/{} expected, {} unexpected | {}",
            entry.aggregate_score * 100.0,
            entry.detection_score * 100.0,
            entry.character_score * 100.0,
            entry.metadata_score * 100.0,
            entry.matched_detection_count,
            entry.expected_detection_count,
            entry.unexpected_actual_count,
            entry.eval
        );
    }
}

/// Manual label file for one full-frame real image.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct EvalSpec {
    pub schema: u32,
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default)]
    pub source_capture: Option<String>,
    pub detections: Vec<ExpectedDetection>,
    #[serde(default)]
    pub ignored: Vec<IgnoredText>,
    #[serde(default)]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ExpectedDetection {
    pub id: String,
    pub bounds: Rect,
    #[serde(default)]
    pub bounds_tolerance: Option<BoundsTolerance>,
    pub text: String,
    pub characters: Vec<ExpectedCharacter>,
    pub tokens: Vec<ExpectedToken>,
    #[serde(default)]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BoundsTolerance {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct BoundsDelta {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ExpectedCharacter {
    pub text: String,
    pub bounds: Rect,
    pub token_id: String,
    #[serde(default)]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CharSpan {
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ExpectedToken {
    pub id: String,
    pub span: CharSpan,
    pub surface: String,
    pub dictionary_form: String,
    pub known: bool,
    pub category: WordCategory,
    pub reasons: Vec<String>,
    pub part_of_speech: Vec<String>,
    pub furigana: Vec<FuriSegment>,
    #[serde(default)]
    pub note: Option<String>,
    pub glosses: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct IgnoredText {
    pub text: String,
    pub reason: String,
    #[serde(default)]
    pub bounds: Option<Rect>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EvalReport {
    pub artifact_version: u32,
    pub image: String,
    pub eval: String,
    pub expected_detection_count: usize,
    pub actual_detection_count: usize,
    pub matched_detection_count: usize,
    pub ignored_actual_count: usize,
    pub unexpected_actual_count: usize,
    pub aggregate_score: f32,
    pub detection_score: f32,
    pub character_score: f32,
    pub metadata_score: f32,
    pub passed: bool,
    pub detections: Vec<DetectionScore>,
    pub ignored_actual: Vec<ActualLineSummary>,
    pub unexpected_actual: Vec<ActualLineSummary>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DetectionScore {
    pub id: String,
    pub expected_bounds: Rect,
    pub expected_text: String,
    pub actual: Option<ActualLineSummary>,
    pub best_iou: f32,
    pub matched_iou: Option<f32>,
    pub bounds_tolerance: BoundsTolerance,
    pub bounds_delta: Option<BoundsDelta>,
    pub detection_score: f32,
    pub char_edit_distance: usize,
    pub character_score: f32,
    pub character_differences: Vec<CharacterDifference>,
    pub token_scores: Vec<TokenScore>,
    pub metadata_score: f32,
    pub score: f32,
}

#[derive(Debug, Clone, Serialize)]
pub struct ActualLineSummary {
    pub text_box: TextBox,
    pub text: String,
    pub confidence: f32,
    pub block_id: usize,
    pub block_text: String,
    pub block_span: TextSpan,
    pub tokens: Vec<ActualTokenSummary>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ActualTokenSummary {
    pub span: CharSpan,
    pub block_span: TextSpan,
    pub surface: String,
    pub full_surface: String,
    pub dictionary_form: String,
    pub known: bool,
    pub category: WordCategory,
    pub reasons: Vec<String>,
    pub part_of_speech: Vec<String>,
    pub furigana: Vec<FuriSegment>,
    pub note: Option<String>,
    pub glosses: Vec<String>,
    pub wraps_before: bool,
    pub wraps_after: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct CharacterDifference {
    pub index: usize,
    pub expected: Option<String>,
    pub actual: Option<String>,
    pub kind: CharacterDifferenceKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CharacterDifferenceKind {
    Mismatch,
    Missing,
    Extra,
}

#[derive(Debug, Clone, Serialize)]
pub struct TokenScore {
    pub id: String,
    pub span: CharSpan,
    pub matched: bool,
    pub expected: ExpectedToken,
    pub actual: Option<ActualTokenSummary>,
    pub field_scores: Vec<FieldScore>,
    pub metadata_score: f32,
}

#[derive(Debug, Clone, Serialize)]
pub struct FieldScore {
    pub field: &'static str,
    pub passed: bool,
    pub expected: Value,
    pub actual: Value,
}

/// Run OCR + lookup on `image`, then score against manual labels from
/// `eval_path`.
pub fn run_eval(
    config: &AppConfig,
    image: &Path,
    eval_path: &Path,
    lexicon: &Path,
    options: EvalOptions,
) -> Result<EvalReport> {
    let spec = load_eval_spec(eval_path)?;
    validate_eval_spec(&spec)?;

    let result = run_ocr_lookup(config, image, lexicon)?;
    Ok(score_ocr_lookup(
        image,
        eval_path,
        &spec,
        &result,
        options.min_iou,
    ))
}

/// Score a previously computed OCR + lookup result against manual labels.
///
/// The interactive eval UI uses this to run OCR once, cache the raw detections,
/// and compare them with edited labels without forcing another inference pass.
pub fn score_ocr_lookup(
    image: &Path,
    eval_path: &Path,
    spec: &EvalSpec,
    result: &OcrLookupResult,
    min_iou: f32,
) -> EvalReport {
    score_eval(image, eval_path, spec, result, min_iou)
}

pub fn load_eval_spec(path: &Path) -> Result<EvalSpec> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read eval labels {}", path.display()))?;
    serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse eval labels {}", path.display()))
}

pub fn validate_eval_spec(spec: &EvalSpec) -> Result<()> {
    if spec.schema != 1 {
        bail!("unsupported eval schema {}; expected 1", spec.schema);
    }

    let mut detection_ids = HashSet::new();
    for detection in &spec.detections {
        if detection.id.trim().is_empty() {
            bail!("detection id must not be empty");
        }
        if !detection_ids.insert(detection.id.as_str()) {
            bail!("duplicate detection id {}", detection.id);
        }
        if detection.text.is_empty() {
            bail!("detection {} text must not be empty", detection.id);
        }
        if detection.characters.is_empty() {
            bail!("detection {} characters must not be empty", detection.id);
        }
        if detection.tokens.is_empty() {
            bail!("detection {} tokens must not be empty", detection.id);
        }
        if detection.bounds.width <= 0.0 || detection.bounds.height <= 0.0 {
            bail!("detection {} has non-positive bounds", detection.id);
        }
        if let Some(tolerance) = detection.bounds_tolerance {
            validate_bounds_tolerance(&detection.id, tolerance)?;
        }
        for (index, ch) in detection.characters.iter().enumerate() {
            if ch.text.chars().count() != 1 {
                bail!(
                    "detection {} character {} must contain exactly one character, got {:?}",
                    detection.id,
                    index,
                    ch.text
                );
            }
            if ch.bounds.width <= 0.0 || ch.bounds.height <= 0.0 {
                bail!(
                    "detection {} character {} has non-positive bounds",
                    detection.id,
                    index
                );
            }
            if !contains_rect(detection.bounds, ch.bounds) {
                bail!(
                    "detection {} character {} bounds must be inside detection bounds",
                    detection.id,
                    index
                );
            }
            if ch.token_id.trim().is_empty() {
                bail!(
                    "detection {} character {} token_id must not be empty",
                    detection.id,
                    index
                );
            }
        }

        let char_text = detection
            .characters
            .iter()
            .map(|ch| ch.text.as_str())
            .collect::<String>();
        if char_text != detection.text {
            bail!(
                "detection {} characters do not exactly spell text: chars={:?} text={:?}",
                detection.id,
                char_text,
                detection.text
            );
        }

        let char_count = detection.characters.len();
        let mut token_ids = HashSet::new();
        let mut token_coverage = vec![0usize; char_count];
        for token in &detection.tokens {
            if token.id.trim().is_empty() {
                bail!("detection {} token id must not be empty", detection.id);
            }
            if !token_ids.insert(token.id.as_str()) {
                bail!("detection {} duplicate token id {}", detection.id, token.id);
            }
            if token.span.start >= token.span.end || token.span.end > char_count {
                bail!(
                    "detection {} token {} has invalid span {}..{} for {} characters",
                    detection.id,
                    token.id,
                    token.span.start,
                    token.span.end,
                    char_count
                );
            }
            let surface = detection.characters[token.span.start..token.span.end]
                .iter()
                .map(|ch| ch.text.as_str())
                .collect::<String>();
            if surface != token.surface {
                bail!(
                    "detection {} token {} surface {:?} does not match character span {:?}",
                    detection.id,
                    token.id,
                    token.surface,
                    surface
                );
            }
            for coverage in &mut token_coverage[token.span.start..token.span.end] {
                *coverage += 1;
            }
            if token.surface.is_empty() {
                bail!(
                    "detection {} token {} surface must not be empty",
                    detection.id,
                    token.id
                );
            }
        }

        for (index, ch) in detection.characters.iter().enumerate() {
            let Some(token) = detection
                .tokens
                .iter()
                .find(|token| token.id == ch.token_id)
            else {
                bail!(
                    "detection {} character {} references unknown token {}",
                    detection.id,
                    index,
                    ch.token_id
                );
            };
            if index < token.span.start || index >= token.span.end {
                bail!(
                    "detection {} character {} references token {} outside its span",
                    detection.id,
                    index,
                    ch.token_id
                );
            }
        }
        for (index, coverage) in token_coverage.iter().enumerate() {
            if *coverage != 1 {
                bail!(
                    "detection {} character {} must be covered by exactly one token span, got {}",
                    detection.id,
                    index,
                    coverage
                );
            }
        }
    }

    for (index, ignored) in spec.ignored.iter().enumerate() {
        if ignored.reason.trim().is_empty() {
            bail!("ignored text {} reason must not be empty", index);
        }
        if ignored.text.is_empty() && ignored.bounds.is_none() {
            bail!("ignored text {} must include text, bounds, or both", index);
        }
        if let Some(bounds) = ignored.bounds
            && (bounds.width <= 0.0 || bounds.height <= 0.0)
        {
            bail!("ignored text {} has non-positive bounds", index);
        }
    }

    Ok(())
}

/// Build one valid manual detection from a text string and a bounding rectangle.
///
/// Character bounds are split evenly across `bounds`; token metadata is derived
/// from the same dictionary, category, furigana, and popup logic used by `watch`
/// and `eval`. This is intended for authoring tools where a human draws the
/// text line box and enters the visible text.
///
/// Keep this path aligned with docs/eval-metadata.md. The authored eval metadata
/// is the executable learner-facing matrix: ambiguous forms should receive the
/// same primary interpretation here as they do in the overlay, while explanatory
/// nuance belongs in notes or future alternate-analysis fields.
pub fn draft_expected_detection(
    dict: &Dictionary,
    id: impl Into<String>,
    text: impl Into<String>,
    bounds: Rect,
    bounds_tolerance: Option<BoundsTolerance>,
    notes: Option<String>,
) -> Result<ExpectedDetection> {
    let id = id.into();
    let text = text.into();
    if id.trim().is_empty() {
        bail!("detection id must not be empty");
    }
    if text.is_empty() {
        bail!("detection {id} text must not be empty");
    }
    if bounds.width <= 0.0 || bounds.height <= 0.0 {
        bail!("detection {id} has non-positive bounds");
    }
    if let Some(tolerance) = bounds_tolerance {
        validate_bounds_tolerance(&id, tolerance)?;
    }

    let chars = text.chars().map(|ch| ch.to_string()).collect::<Vec<_>>();
    let token_spans = complete_token_spans(dict, &text);
    let expected_tokens = draft_expected_tokens(dict, &text);

    let char_width = bounds.width / chars.len() as f32;
    let characters = chars
        .into_iter()
        .enumerate()
        .map(|(index, text)| {
            let token_id = token_at_char(
                &token_spans
                    .iter()
                    .map(|draft| (draft.start, draft.end))
                    .collect::<Vec<_>>(),
                index,
            )
            .and_then(|token_index| expected_tokens.get(token_index))
            .map(|token| token.id.clone())
            .unwrap_or_else(|| expected_tokens[0].id.clone());
            ExpectedCharacter {
                text,
                bounds: Rect::new(
                    bounds.x + index as f32 * char_width,
                    bounds.y,
                    char_width,
                    bounds.height,
                ),
                token_id,
                notes: None,
            }
        })
        .collect();

    let detection = ExpectedDetection {
        id,
        bounds,
        bounds_tolerance,
        text,
        characters,
        tokens: expected_tokens,
        notes,
    };
    validate_eval_spec(&EvalSpec {
        schema: 1,
        image: None,
        source_capture: None,
        detections: vec![detection.clone()],
        ignored: Vec::new(),
        notes: None,
    })?;
    Ok(detection)
}

/// The canonical matrix tokenization of `text` as expected-token labels, the
/// same shapes `draft_expected_detection` embeds. Label-audit tooling uses this
/// to regenerate token arrays that must agree with the runtime matrix.
pub fn draft_expected_tokens(dict: &Dictionary, text: &str) -> Vec<ExpectedToken> {
    let token_spans = complete_token_spans(dict, text);
    token_spans
        .iter()
        .enumerate()
        .map(|(index, draft)| {
            expected_token_from_token(
                &draft.token,
                CharSpan {
                    start: draft.start,
                    end: draft.end,
                },
                format!("token_{index:02}"),
                &token_spans,
                index,
            )
        })
        .collect()
}

#[derive(Debug, Clone)]
struct DraftTokenSpan {
    token: Token,
    start: usize,
    end: usize,
}

fn complete_token_spans(dict: &Dictionary, text: &str) -> Vec<DraftTokenSpan> {
    // Wrapped-fragment policy lives in docs/eval-metadata.md: if dictionary
    // analysis can stitch visible chunks into a real word, the fragment inherits
    // that full-word metadata; otherwise uncovered text remains an unknown
    // surface token instead of inventing invisible continuation text.
    let chars = text.chars().map(|ch| ch.to_string()).collect::<Vec<_>>();
    let char_count = chars.len();
    let analyzed = dict.analyze_line(text);
    let spans = token_spans(text, &analyzed);
    let mut drafts = analyzed
        .into_iter()
        .zip(spans)
        .filter(|(_, (start, end))| start < end && *end <= char_count)
        .map(|(token, (start, end))| DraftTokenSpan { token, start, end })
        .collect::<Vec<_>>();

    let mut covered = vec![false; char_count];
    for draft in &drafts {
        for slot in &mut covered[draft.start..draft.end] {
            *slot = true;
        }
    }

    let mut index = 0;
    while index < char_count {
        if covered[index] {
            index += 1;
            continue;
        }
        let start = index;
        while index < char_count && !covered[index] {
            index += 1;
        }
        let surface = chars[start..index].join("");
        drafts.push(DraftTokenSpan {
            token: Token {
                surface: surface.clone(),
                dictionary_form: surface,
                reasons: Vec::new(),
                entries: Vec::new(),
                source_pos: None,
                note_override: None,
            },
            start,
            end: index,
        });
    }

    if drafts.is_empty() {
        let surface = text.to_string();
        drafts.push(DraftTokenSpan {
            token: Token {
                surface: surface.clone(),
                dictionary_form: surface,
                reasons: Vec::new(),
                entries: Vec::new(),
                source_pos: None,
                note_override: None,
            },
            start: 0,
            end: char_count,
        });
    }

    drafts.sort_by_key(|draft| (draft.start, draft.end));
    drafts
}

fn expected_token_from_token(
    token: &Token,
    span: CharSpan,
    id: String,
    tokens: &[DraftTokenSpan],
    index: usize,
) -> ExpectedToken {
    let token_values = tokens
        .iter()
        .map(|draft| draft.token.clone())
        .collect::<Vec<_>>();
    let hint = transitivity_hint(&token_values, index);
    let content = popup_content(token, hint, DEFAULT_MAX_SENSES, DEFAULT_MAX_GLOSSES);
    let part_of_speech = token
        .entries
        .first()
        .and_then(|entry| entry.senses.first())
        .map(|sense| sense.part_of_speech.clone())
        .unwrap_or_default();
    ExpectedToken {
        id,
        span,
        surface: token.surface.clone(),
        dictionary_form: token.dictionary_form.clone(),
        known: token.is_known(),
        category: categorize(token),
        reasons: token.reasons.clone(),
        part_of_speech,
        furigana: content.ruby,
        note: content.note,
        glosses: content.glosses,
    }
}

fn validate_bounds_tolerance(detection_id: &str, tolerance: BoundsTolerance) -> Result<()> {
    for (field, value) in [
        ("x", tolerance.x),
        ("y", tolerance.y),
        ("width", tolerance.width),
        ("height", tolerance.height),
    ] {
        if !value.is_finite() || value < 0.0 {
            bail!(
                "detection {} bounds_tolerance.{} must be a finite non-negative number",
                detection_id,
                field
            );
        }
    }
    Ok(())
}

fn score_eval(
    image: &Path,
    eval_path: &Path,
    spec: &EvalSpec,
    result: &OcrLookupResult,
    min_iou: f32,
) -> EvalReport {
    let matches = match_detections(&spec.detections, &result.lines, min_iou);
    let mut detections = Vec::with_capacity(spec.detections.len());
    let mut matched_actual = HashSet::new();

    for (expected_index, expected) in spec.detections.iter().enumerate() {
        let matched = matches[expected_index];
        if let Some(actual_index) = matched {
            matched_actual.insert(actual_index);
        }
        detections.push(score_detection(
            expected,
            matched.map(|index| &result.lines[index]),
            result
                .lines
                .iter()
                .map(|line| expected.bounds.iou(line.text_box.rect))
                .fold(0.0, f32::max),
        ));
    }

    let mut ignored_actual = Vec::new();
    let mut unexpected_actual = Vec::new();
    for line in result
        .lines
        .iter()
        .enumerate()
        .filter(|(index, _)| !matched_actual.contains(index))
        .map(|(_, line)| line)
    {
        if is_ignored_actual(line, &spec.ignored, min_iou) {
            ignored_actual.push(actual_line_summary(line));
        } else {
            unexpected_actual.push(actual_line_summary(line));
        }
    }

    let expected_detection_count = spec.detections.len();
    let actual_detection_count = result.lines.len();
    let matched_detection_count = matches.iter().filter(|entry| entry.is_some()).count();
    let ignored_actual_count = ignored_actual.len();
    let unexpected_actual_count = unexpected_actual.len();

    let detection_denominator = expected_detection_count + unexpected_actual_count;
    let detection_score = if detection_denominator == 0 {
        1.0
    } else {
        detections
            .iter()
            .map(|score| score.detection_score)
            .sum::<f32>()
            / detection_denominator as f32
    };

    let unexpected_character_count = unexpected_actual
        .iter()
        .map(|actual| actual.text.chars().count())
        .sum::<usize>();
    let character_error_units = detections
        .iter()
        .map(|score| score.char_edit_distance)
        .sum::<usize>()
        + unexpected_character_count;
    let character_denominator = detections
        .iter()
        .map(|score| {
            let expected_len = score.expected_text.chars().count();
            let actual_len = score
                .actual
                .as_ref()
                .map(|actual| actual.text.chars().count())
                .unwrap_or(0);
            expected_len.max(actual_len)
        })
        .sum::<usize>()
        + unexpected_character_count;
    let character_score = if character_denominator == 0 {
        1.0
    } else {
        1.0 - (character_error_units as f32 / character_denominator as f32)
    };

    let expected_tokens: usize = detections
        .iter()
        .map(|detection| detection.token_scores.len())
        .sum();
    let unexpected_tokens = unexpected_actual
        .iter()
        .map(|actual| actual.tokens.len().max(1))
        .sum::<usize>();
    let metadata_denominator = expected_tokens + unexpected_tokens;
    let metadata_score = if metadata_denominator == 0 {
        1.0
    } else {
        detections
            .iter()
            .map(|score| score.metadata_score * score.token_scores.len() as f32)
            .sum::<f32>()
            / metadata_denominator as f32
    };

    let aggregate_score = detection_score * AGGREGATE_DETECTION_WEIGHT
        + character_score * AGGREGATE_CHARACTER_WEIGHT
        + metadata_score * AGGREGATE_METADATA_WEIGHT;
    let passed = aggregate_score >= 1.0 - PERFECT_SCORE_EPSILON;

    EvalReport {
        artifact_version: crate::artifact::ARTIFACT_VERSION,
        image: image.display().to_string(),
        eval: eval_path.display().to_string(),
        expected_detection_count,
        actual_detection_count,
        matched_detection_count,
        ignored_actual_count,
        unexpected_actual_count,
        aggregate_score,
        detection_score,
        character_score,
        metadata_score,
        passed,
        detections,
        ignored_actual,
        unexpected_actual,
    }
}

fn is_ignored_actual(line: &LineLookup, ignored: &[IgnoredText], min_iou: f32) -> bool {
    ignored.iter().any(|ignored| match ignored.bounds {
        Some(bounds) => {
            rect_coverage(line.text_box.rect, bounds) >= min_iou
                || line.text_box.rect.iou(bounds) >= min_iou
        }
        None => line.text == ignored.text,
    })
}

fn rect_coverage(subject: Rect, cover: Rect) -> f32 {
    let intersection = intersection_area(subject, cover);
    if intersection <= 0.0 || subject.area() <= 0.0 {
        0.0
    } else {
        intersection / subject.area()
    }
}

fn contains_rect(outer: Rect, inner: Rect) -> bool {
    const EPSILON: f32 = 0.5;
    inner.x + EPSILON >= outer.x
        && inner.y + EPSILON >= outer.y
        && inner.right() <= outer.right() + EPSILON
        && inner.bottom() <= outer.bottom() + EPSILON
}

fn intersection_area(a: Rect, b: Rect) -> f32 {
    let left = a.x.max(b.x);
    let top = a.y.max(b.y);
    let right = a.right().min(b.right());
    let bottom = a.bottom().min(b.bottom());
    Rect::new(left, top, right - left, bottom - top).area()
}

#[derive(Debug, Clone, Copy)]
struct DetectionGeometry {
    iou: f32,
    score: f32,
    within_free_tolerance: bool,
    center_in_expanded_bounds: bool,
    coverage: f32,
    delta: BoundsDelta,
}

impl DetectionGeometry {
    fn matches(self, min_iou: f32) -> bool {
        self.iou >= min_iou
            || self.within_free_tolerance
            || (self.center_in_expanded_bounds && self.coverage >= TOLERANT_MATCH_MIN_COVERAGE)
    }
}

fn detection_geometry(expected: &ExpectedDetection, actual: Rect) -> DetectionGeometry {
    let tolerance = expected
        .bounds_tolerance
        .unwrap_or_else(|| default_bounds_tolerance(expected.bounds));
    let delta = bounds_delta(expected.bounds, actual);
    let within_free_tolerance = delta.x <= tolerance.x
        && delta.y <= tolerance.y
        && delta.width <= tolerance.width
        && delta.height <= tolerance.height;
    let center = rect_center(actual);
    let expanded = expand_rect(
        expected.bounds,
        tolerance.scale(BOUNDS_ZERO_SCORE_MULTIPLIER),
    );
    let coverage =
        rect_coverage(expected.bounds, actual).max(rect_coverage(actual, expected.bounds));
    DetectionGeometry {
        iou: expected.bounds.iou(actual),
        score: bounds_score(delta, tolerance),
        within_free_tolerance,
        center_in_expanded_bounds: expanded.contains(center.0, center.1),
        coverage,
        delta,
    }
}

fn default_bounds_tolerance(bounds: Rect) -> BoundsTolerance {
    let height = bounds.height.max(1.0);
    BoundsTolerance {
        x: DEFAULT_BOUNDS_POSITION_TOLERANCE_PX
            .max(height * DEFAULT_BOUNDS_POSITION_TOLERANCE_HEIGHT_RATIO),
        y: DEFAULT_BOUNDS_POSITION_TOLERANCE_PX
            .max(height * DEFAULT_BOUNDS_POSITION_TOLERANCE_HEIGHT_RATIO),
        width: DEFAULT_BOUNDS_SIZE_TOLERANCE_PX
            .max(height * DEFAULT_BOUNDS_SIZE_TOLERANCE_HEIGHT_RATIO),
        height: DEFAULT_BOUNDS_SIZE_TOLERANCE_PX
            .max(height * DEFAULT_BOUNDS_SIZE_TOLERANCE_HEIGHT_RATIO),
    }
}

impl BoundsTolerance {
    fn scale(self, factor: f32) -> Self {
        Self {
            x: self.x * factor,
            y: self.y * factor,
            width: self.width * factor,
            height: self.height * factor,
        }
    }
}

fn bounds_delta(expected: Rect, actual: Rect) -> BoundsDelta {
    BoundsDelta {
        x: (actual.x - expected.x).abs(),
        y: (actual.y - expected.y).abs(),
        width: (actual.width - expected.width).abs(),
        height: (actual.height - expected.height).abs(),
    }
}

fn bounds_score(delta: BoundsDelta, tolerance: BoundsTolerance) -> f32 {
    let axis_scores = [
        tolerance_axis_score(delta.x, tolerance.x),
        tolerance_axis_score(delta.y, tolerance.y),
        tolerance_axis_score(delta.width, tolerance.width),
        tolerance_axis_score(delta.height, tolerance.height),
    ];
    axis_scores.iter().sum::<f32>() / axis_scores.len() as f32
}

fn tolerance_axis_score(delta: f32, free_tolerance: f32) -> f32 {
    if delta <= free_tolerance {
        return 1.0;
    }
    let zero_delta = free_tolerance * BOUNDS_ZERO_SCORE_MULTIPLIER;
    if zero_delta <= free_tolerance || delta >= zero_delta {
        return 0.0;
    }
    1.0 - ((delta - free_tolerance) / (zero_delta - free_tolerance))
}

fn expand_rect(rect: Rect, tolerance: BoundsTolerance) -> Rect {
    Rect::new(
        rect.x - tolerance.x,
        rect.y - tolerance.y,
        rect.width + tolerance.x + tolerance.width,
        rect.height + tolerance.y + tolerance.height,
    )
}

fn rect_center(rect: Rect) -> (f32, f32) {
    (rect.x + rect.width / 2.0, rect.y + rect.height / 2.0)
}

fn match_detections(
    expected: &[ExpectedDetection],
    actual: &[LineLookup],
    min_iou: f32,
) -> Vec<Option<usize>> {
    let mut pairs = Vec::new();
    for (expected_index, detection) in expected.iter().enumerate() {
        for (actual_index, line) in actual.iter().enumerate() {
            let geometry = detection_geometry(detection, line.text_box.rect);
            if geometry.matches(min_iou) {
                pairs.push((geometry.score, geometry.iou, expected_index, actual_index));
            }
        }
    }
    pairs.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| b.1.total_cmp(&a.1)));

    let mut matches = vec![None; expected.len()];
    let mut used_actual = HashSet::new();
    for (_, _, expected_index, actual_index) in pairs {
        if matches[expected_index].is_none() && used_actual.insert(actual_index) {
            matches[expected_index] = Some(actual_index);
        }
    }
    matches
}

fn score_detection(
    expected: &ExpectedDetection,
    actual: Option<&LineLookup>,
    best_iou: f32,
) -> DetectionScore {
    let tolerance = expected
        .bounds_tolerance
        .unwrap_or_else(|| default_bounds_tolerance(expected.bounds));
    let (actual_summary, matched_iou, bounds_delta, detection_score, actual_text, actual_tokens) =
        match actual {
            Some(line) => {
                let geometry = detection_geometry(expected, line.text_box.rect);
                (
                    Some(actual_line_summary(line)),
                    Some(geometry.iou),
                    Some(geometry.delta),
                    geometry.score,
                    line.text.clone(),
                    actual_token_summaries(line),
                )
            }
            None => (None, None, None, 0.0, String::new(), Vec::new()),
        };

    let expected_chars: Vec<String> = expected
        .characters
        .iter()
        .map(|ch| ch.text.clone())
        .collect();
    let actual_chars: Vec<String> = actual_text.chars().map(|ch| ch.to_string()).collect();
    let char_edit_distance = edit_distance(&expected_chars, &actual_chars);
    let character_score = sequence_accuracy(&expected_chars, &actual_chars);
    let character_differences = character_differences(&expected_chars, &actual_chars);

    let token_scores = expected
        .tokens
        .iter()
        .filter(|token| !is_layout_metadata_token(token))
        .map(|token| score_token(token, &actual_tokens))
        .collect::<Vec<_>>();
    let metadata_score = if token_scores.is_empty() {
        1.0
    } else {
        token_scores
            .iter()
            .map(|score| score.metadata_score)
            .sum::<f32>()
            / token_scores.len() as f32
    };
    let score = detection_score * AGGREGATE_DETECTION_WEIGHT
        + character_score * AGGREGATE_CHARACTER_WEIGHT
        + metadata_score * AGGREGATE_METADATA_WEIGHT;

    DetectionScore {
        id: expected.id.clone(),
        expected_bounds: expected.bounds,
        expected_text: expected.text.clone(),
        actual: actual_summary,
        best_iou,
        matched_iou,
        bounds_tolerance: tolerance,
        bounds_delta,
        detection_score,
        char_edit_distance,
        character_score,
        character_differences,
        token_scores,
        metadata_score,
        score,
    }
}

fn is_layout_metadata_token(token: &ExpectedToken) -> bool {
    token.surface.chars().all(char::is_whitespace) || matches!(token.surface.as_str(), ":" | "：")
}

fn score_token(expected: &ExpectedToken, actual_tokens: &[ActualTokenSummary]) -> TokenScore {
    let actual = actual_tokens
        .iter()
        .find(|token| token.span == expected.span)
        .cloned();
    let field_scores = match &actual {
        Some(actual) => {
            let mut scores = vec![
                field_score("surface", &expected.surface, &actual.surface),
                field_score(
                    "dictionary_form",
                    &expected.dictionary_form,
                    &actual.dictionary_form,
                ),
                field_score("known", &expected.known, &actual.known),
                field_score("category", &expected.category, &actual.category),
                field_score("reasons", &expected.reasons, &actual.reasons),
                field_score(
                    "part_of_speech",
                    &expected.part_of_speech,
                    &actual.part_of_speech,
                ),
                field_score("furigana", &expected.furigana, &actual.furigana),
            ];
            if should_score_expected_note(expected) {
                scores.push(field_score("note", &expected.note, &actual.note));
            }
            scores.push(field_score("glosses", &expected.glosses, &actual.glosses));
            scores
        }
        None => vec![FieldScore {
            field: "span",
            passed: false,
            expected: value(&expected.span),
            actual: Value::Null,
        }],
    };
    let metadata_score = if field_scores.is_empty() {
        1.0
    } else {
        field_scores.iter().filter(|score| score.passed).count() as f32 / field_scores.len() as f32
    };

    TokenScore {
        id: expected.id.clone(),
        span: expected.span,
        matched: actual.is_some(),
        expected: expected.clone(),
        actual,
        field_scores,
        metadata_score,
    }
}

fn should_score_expected_note(expected: &ExpectedToken) -> bool {
    expected.note == runtime_note_from_expected_token(expected)
}

fn runtime_note_from_expected_token(expected: &ExpectedToken) -> Option<String> {
    if expected.surface != expected.dictionary_form {
        let reasons = if expected.reasons.is_empty() {
            String::new()
        } else {
            format!(" · {}", expected.reasons.join(" < "))
        };
        Some(format!("{}{reasons}", expected.surface))
    } else if !expected.reasons.is_empty() {
        Some(expected.reasons.join(" < "))
    } else {
        None
    }
}

fn actual_line_summary(line: &LineLookup) -> ActualLineSummary {
    ActualLineSummary {
        text_box: line.text_box.clone(),
        text: line.text.clone(),
        confidence: line.confidence,
        block_id: line.block_id,
        block_text: line.block_text.clone(),
        block_span: line.block_span,
        tokens: actual_token_summaries(line),
    }
}

fn actual_token_summaries(line: &LineLookup) -> Vec<ActualTokenSummary> {
    line.tokens
        .iter()
        .map(|segment| {
            let hint = transitivity_hint(&line.block_tokens, segment.block_token_index);
            let content = popup_content(
                &segment.token,
                hint,
                DEFAULT_MAX_SENSES,
                DEFAULT_MAX_GLOSSES,
            );
            let part_of_speech = segment
                .token
                .entries
                .first()
                .and_then(|entry| entry.senses.first())
                .map(|sense| sense.part_of_speech.clone())
                .unwrap_or_default();
            ActualTokenSummary {
                span: CharSpan {
                    start: segment.span.start,
                    end: segment.span.end,
                },
                block_span: segment.block_span,
                surface: segment.visible_surface.clone(),
                full_surface: segment.token.surface.clone(),
                dictionary_form: segment.token.dictionary_form.clone(),
                known: segment.token.is_known(),
                category: categorize(&segment.token),
                reasons: segment.token.reasons.clone(),
                part_of_speech,
                furigana: content.ruby,
                note: content.note,
                glosses: content.glosses,
                wraps_before: segment.wraps_before,
                wraps_after: segment.wraps_after,
            }
        })
        .collect()
}

fn field_score<T>(field: &'static str, expected: &T, actual: &T) -> FieldScore
where
    T: Serialize + PartialEq,
{
    FieldScore {
        field,
        passed: expected == actual,
        expected: value(expected),
        actual: value(actual),
    }
}

fn value(value: &impl Serialize) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

fn sequence_accuracy(expected: &[String], actual: &[String]) -> f32 {
    let denom = expected.len().max(actual.len());
    if denom == 0 {
        return 1.0;
    }
    1.0 - (edit_distance(expected, actual) as f32 / denom as f32)
}

fn edit_distance(expected: &[String], actual: &[String]) -> usize {
    let mut previous: Vec<usize> = (0..=actual.len()).collect();
    let mut current = vec![0; actual.len() + 1];
    for (i, expected_ch) in expected.iter().enumerate() {
        current[0] = i + 1;
        for (j, actual_ch) in actual.iter().enumerate() {
            let substitution = previous[j] + usize::from(expected_ch != actual_ch);
            let insertion = current[j] + 1;
            let deletion = previous[j + 1] + 1;
            current[j + 1] = substitution.min(insertion).min(deletion);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[actual.len()]
}

fn character_differences(expected: &[String], actual: &[String]) -> Vec<CharacterDifference> {
    let max_len = expected.len().max(actual.len());
    (0..max_len)
        .filter_map(|index| match (expected.get(index), actual.get(index)) {
            (Some(expected), Some(actual)) if expected == actual => None,
            (Some(expected), Some(actual)) => Some(CharacterDifference {
                index,
                expected: Some(expected.clone()),
                actual: Some(actual.clone()),
                kind: CharacterDifferenceKind::Mismatch,
            }),
            (Some(expected), None) => Some(CharacterDifference {
                index,
                expected: Some(expected.clone()),
                actual: None,
                kind: CharacterDifferenceKind::Missing,
            }),
            (None, Some(actual)) => Some(CharacterDifference {
                index,
                expected: None,
                actual: Some(actual.clone()),
                kind: CharacterDifferenceKind::Extra,
            }),
            (None, None) => None,
        })
        .collect()
}

pub fn render_eval_report(report: &EvalReport) {
    println!(
        "eval: aggregate {:.1}% | detection {:.1}% | characters {:.1}% | metadata {:.1}% | matched {}/{} expected, {} actual, {} unexpected, {} ignored",
        report.aggregate_score * 100.0,
        report.detection_score * 100.0,
        report.character_score * 100.0,
        report.metadata_score * 100.0,
        report.matched_detection_count,
        report.expected_detection_count,
        report.actual_detection_count,
        report.unexpected_actual_count,
        report.ignored_actual_count,
    );

    for detection in &report.detections {
        let mark = if detection.score >= 1.0 - PERFECT_SCORE_EPSILON {
            "PASS"
        } else {
            "FAIL"
        };
        println!(
            "[{mark}] {} score {:.1}% bounds {:.1}% iou {:.1}% chars {:.1}% metadata {:.1}%",
            detection.id,
            detection.score * 100.0,
            detection.detection_score * 100.0,
            detection.matched_iou.unwrap_or(detection.best_iou) * 100.0,
            detection.character_score * 100.0,
            detection.metadata_score * 100.0,
        );
        println!("  expected: {}", detection.expected_text);
        match &detection.actual {
            Some(actual) => println!("  actual:   {}", actual.text),
            None => println!("  actual:   <no matched detection>"),
        }
        if detection.detection_score < 1.0
            && let Some(delta) = detection.bounds_delta
        {
            println!(
                "  bounds delta: x={:.1} y={:.1} width={:.1} height={:.1}; tolerance: x={:.1} y={:.1} width={:.1} height={:.1}",
                delta.x,
                delta.y,
                delta.width,
                delta.height,
                detection.bounds_tolerance.x,
                detection.bounds_tolerance.y,
                detection.bounds_tolerance.width,
                detection.bounds_tolerance.height,
            );
        }
        if !detection.character_differences.is_empty() {
            let preview = detection
                .character_differences
                .iter()
                .take(8)
                .map(|diff| {
                    format!(
                        "#{} {:?}->{:?} ({:?})",
                        diff.index, diff.expected, diff.actual, diff.kind
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            println!("  character differences: {preview}");
        }
        let failed_tokens = detection
            .token_scores
            .iter()
            .filter(|token| token.metadata_score < 1.0)
            .collect::<Vec<_>>();
        if !failed_tokens.is_empty() {
            println!(
                "  token metadata failures: {}",
                failed_tokens
                    .iter()
                    .map(|token| token.id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
    }

    if !report.unexpected_actual.is_empty() {
        println!(
            "unexpected actual detections: {}",
            report.unexpected_actual.len()
        );
        for actual in report.unexpected_actual.iter().take(10) {
            println!(
                "  x={:.0} y={:.0} w={:.0} h={:.0}: {}",
                actual.text_box.rect.x,
                actual.text_box.rect.y,
                actual.text_box.rect.width,
                actual.text_box.rect.height,
                actual.text
            );
        }
    }
    if !report.ignored_actual.is_empty() {
        println!("ignored actual detections: {}", report.ignored_actual.len());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dictionary::{Entry, Sense};

    fn rect(x: f32, y: f32, width: f32, height: f32) -> Rect {
        Rect::new(x, y, width, height)
    }

    fn text_box(rect: Rect) -> TextBox {
        TextBox {
            id: 0,
            rect,
            confidence: 0.95,
            content_fingerprint: 42,
        }
    }

    fn entry(kanji: &[&str], kana: &[&str], pos: &str, glosses: &[&str]) -> Entry {
        Entry {
            kanji: kanji.iter().map(|s| s.to_string()).collect(),
            kana: kana.iter().map(|s| s.to_string()).collect(),
            senses: vec![Sense {
                part_of_speech: vec![pos.to_string()],
                glosses: glosses.iter().map(|s| s.to_string()).collect(),
                misc: Vec::new(),
            }],
            common: true,
            popup_override: None,
        }
    }

    fn token(surface: &str, dictionary_form: &str, entry: Entry) -> Token {
        Token {
            surface: surface.to_string(),
            dictionary_form: dictionary_form.to_string(),
            reasons: Vec::new(),
            entries: vec![entry],
            source_pos: None,
            note_override: None,
        }
    }

    fn line_token(surface: &str, token: Token, start: usize, end: usize) -> LineToken {
        LineToken {
            span: TextSpan { start, end },
            block_span: TextSpan { start, end },
            visible_surface: surface.to_string(),
            token,
            block_token_index: 0,
            wraps_before: false,
            wraps_after: false,
        }
    }

    fn actual_token(surface: &str, start: usize, end: usize) -> ActualTokenSummary {
        ActualTokenSummary {
            span: CharSpan { start, end },
            block_span: TextSpan { start, end },
            surface: surface.to_string(),
            full_surface: surface.to_string(),
            dictionary_form: surface.to_string(),
            known: false,
            category: WordCategory::Unknown,
            reasons: Vec::new(),
            part_of_speech: Vec::new(),
            furigana: vec![FuriSegment {
                text: surface.to_string(),
                furigana: None,
            }],
            note: None,
            glosses: Vec::new(),
            wraps_before: false,
            wraps_after: false,
        }
    }

    fn expected_unknown_token(id: &str, surface: &str, start: usize, end: usize) -> ExpectedToken {
        ExpectedToken {
            id: id.to_string(),
            span: CharSpan { start, end },
            surface: surface.to_string(),
            dictionary_form: surface.to_string(),
            known: false,
            category: WordCategory::Unknown,
            reasons: Vec::new(),
            part_of_speech: Vec::new(),
            furigana: vec![FuriSegment {
                text: surface.to_string(),
                furigana: None,
            }],
            note: None,
            glosses: Vec::new(),
        }
    }

    fn water_spec() -> EvalSpec {
        EvalSpec {
            schema: 1,
            image: None,
            source_capture: None,
            detections: vec![ExpectedDetection {
                id: "water".to_string(),
                bounds: rect(10.0, 20.0, 30.0, 40.0),
                bounds_tolerance: None,
                text: "水".to_string(),
                characters: vec![ExpectedCharacter {
                    text: "水".to_string(),
                    bounds: rect(10.0, 20.0, 30.0, 40.0),
                    token_id: "water_token".to_string(),
                    notes: None,
                }],
                tokens: vec![ExpectedToken {
                    id: "water_token".to_string(),
                    span: CharSpan { start: 0, end: 1 },
                    surface: "水".to_string(),
                    dictionary_form: "水".to_string(),
                    known: true,
                    category: WordCategory::Noun,
                    reasons: Vec::new(),
                    part_of_speech: vec!["n".to_string()],
                    furigana: vec![FuriSegment {
                        text: "水".to_string(),
                        furigana: Some("みず".to_string()),
                    }],
                    note: None,
                    glosses: vec!["water  (n)".to_string()],
                }],
                notes: None,
            }],
            ignored: Vec::new(),
            notes: None,
        }
    }

    fn water_result(text: &str, rect: Rect) -> OcrLookupResult {
        let token = token("水", "水", entry(&["水"], &["みず"], "n", &["water"]));
        OcrLookupResult {
            image: "image.png".to_string(),
            recognized_text: text.to_string(),
            lines: vec![LineLookup {
                text_box: text_box(rect),
                text: text.to_string(),
                confidence: 0.99,
                reused: false,
                char_centers: vec![25.0],
                block_id: 0,
                block_text: text.to_string(),
                block_span: TextSpan { start: 0, end: 1 },
                block_tokens: vec![token.clone()],
                tokens: vec![line_token("水", token, 0, 1)],
            }],
        }
    }

    fn two_line_result(
        first: &str,
        first_rect: Rect,
        second: &str,
        second_rect: Rect,
    ) -> OcrLookupResult {
        let token = token("水", "水", entry(&["水"], &["みず"], "n", &["water"]));
        OcrLookupResult {
            image: "image.png".to_string(),
            recognized_text: format!("{first} {second}"),
            lines: vec![
                LineLookup {
                    text_box: text_box(first_rect),
                    text: first.to_string(),
                    confidence: 0.99,
                    reused: false,
                    char_centers: vec![25.0],
                    block_id: 0,
                    block_text: first.to_string(),
                    block_span: TextSpan { start: 0, end: 1 },
                    block_tokens: vec![token.clone()],
                    tokens: vec![line_token("水", token, 0, 1)],
                },
                LineLookup {
                    text_box: text_box(second_rect),
                    text: second.to_string(),
                    confidence: 0.99,
                    reused: false,
                    char_centers: vec![125.0],
                    block_id: 1,
                    block_text: second.to_string(),
                    block_span: TextSpan {
                        start: 0,
                        end: second.chars().count(),
                    },
                    block_tokens: Vec::new(),
                    tokens: Vec::new(),
                },
            ],
        }
    }

    #[test]
    fn validation_rejects_character_text_drift() {
        let mut spec = water_spec();
        spec.detections[0].characters[0].text = "氷".to_string();
        let error = validate_eval_spec(&spec).unwrap_err().to_string();
        assert!(error.contains("characters do not exactly spell text"));
    }

    #[test]
    fn validation_rejects_token_surface_drift() {
        let mut spec = water_spec();
        spec.detections[0].tokens[0].surface = "氷".to_string();
        let error = validate_eval_spec(&spec).unwrap_err().to_string();
        assert!(error.contains("does not match character span"));
    }

    #[test]
    fn validation_rejects_negative_bounds_tolerance() {
        let mut spec = water_spec();
        spec.detections[0].bounds_tolerance = Some(BoundsTolerance {
            x: -1.0,
            y: 4.0,
            width: 6.0,
            height: 6.0,
        });
        let error = validate_eval_spec(&spec).unwrap_err().to_string();
        assert!(error.contains("bounds_tolerance.x"));
    }

    #[test]
    fn token_score_excludes_annotation_notes() {
        let mut expected = expected_unknown_token("count", "0/2", 0, 3);
        expected.note = Some("Purchase limit/count value.".to_string());
        let actual = vec![actual_token("0/2", 0, 3)];

        let score = score_token(&expected, &actual);

        assert_eq!(score.metadata_score, 1.0);
        assert!(score.field_scores.iter().all(|field| field.field != "note"));
    }

    #[test]
    fn token_score_keeps_runtime_notes_exact() {
        let mut expected = expected_unknown_token("verb", "し", 0, 1);
        expected.dictionary_form = "する".to_string();
        expected.known = true;
        expected.category = WordCategory::Verb;
        expected.reasons = vec!["連用形".to_string()];
        expected.note = Some("し · 連用形".to_string());
        let mut actual = actual_token("し", 0, 1);
        actual.dictionary_form = "する".to_string();
        actual.known = true;
        actual.category = WordCategory::Verb;
        actual.reasons = vec!["連用形".to_string()];
        actual.note = None;

        let score = score_token(&expected, &[actual]);

        assert!(
            score
                .field_scores
                .iter()
                .any(|field| field.field == "note" && !field.passed)
        );
        assert!(score.metadata_score < 1.0);
    }

    #[test]
    fn layout_separator_tokens_do_not_affect_metadata_score() {
        let mut spec = water_spec();
        let detection = &mut spec.detections[0];
        detection.text = "水 水".to_string();
        detection.bounds = rect(10.0, 20.0, 90.0, 40.0);
        detection.characters = vec![
            ExpectedCharacter {
                text: "水".to_string(),
                bounds: rect(10.0, 20.0, 30.0, 40.0),
                token_id: "water_left".to_string(),
                notes: None,
            },
            ExpectedCharacter {
                text: " ".to_string(),
                bounds: rect(40.0, 20.0, 30.0, 40.0),
                token_id: "space".to_string(),
                notes: None,
            },
            ExpectedCharacter {
                text: "水".to_string(),
                bounds: rect(70.0, 20.0, 30.0, 40.0),
                token_id: "water_right".to_string(),
                notes: None,
            },
        ];
        let mut left = detection.tokens[0].clone();
        left.id = "water_left".to_string();
        left.span = CharSpan { start: 0, end: 1 };
        let space = expected_unknown_token("space", " ", 1, 2);
        let mut right = left.clone();
        right.id = "water_right".to_string();
        right.span = CharSpan { start: 2, end: 3 };
        detection.tokens = vec![left, space, right];

        let water = token("水", "水", entry(&["水"], &["みず"], "n", &["water"]));
        let result = OcrLookupResult {
            image: "image.png".to_string(),
            recognized_text: "水 水".to_string(),
            lines: vec![LineLookup {
                text_box: text_box(detection.bounds),
                text: "水 水".to_string(),
                confidence: 0.99,
                reused: false,
                char_centers: vec![25.0, 85.0],
                block_id: 0,
                block_text: "水 水".to_string(),
                block_span: TextSpan { start: 0, end: 3 },
                block_tokens: vec![water.clone(), water.clone()],
                tokens: vec![
                    line_token("水", water.clone(), 0, 1),
                    line_token("水", water, 2, 3),
                ],
            }],
        };

        let report = score_ocr_lookup(
            Path::new("image.png"),
            Path::new("eval.json"),
            &spec,
            &result,
            0.50,
        );

        assert_eq!(report.detections[0].token_scores.len(), 2);
        assert_eq!(report.detections[0].metadata_score, 1.0);
        assert_eq!(report.metadata_score, 1.0);
    }

    #[test]
    fn perfect_manual_label_scores_full_credit() {
        let spec = water_spec();
        validate_eval_spec(&spec).unwrap();
        let result = water_result("水", rect(10.0, 20.0, 30.0, 40.0));
        let report = score_eval(
            Path::new("image.png"),
            Path::new("eval.json"),
            &spec,
            &result,
            0.50,
        );
        assert!(report.passed);
        assert_eq!(report.matched_detection_count, 1);
        assert!((report.aggregate_score - 1.0).abs() < 0.0001);
    }

    #[test]
    fn small_bounds_jitter_scores_full_credit() {
        let mut spec = water_spec();
        spec.detections[0].bounds = rect(10.0, 20.0, 30.0, 10.0);
        spec.detections[0].characters[0].bounds = rect(10.0, 20.0, 30.0, 10.0);
        let result = water_result("水", rect(10.0, 24.0, 30.0, 10.0));
        let report = score_eval(
            Path::new("image.png"),
            Path::new("eval.json"),
            &spec,
            &result,
            0.50,
        );
        assert!(report.passed);
        assert_eq!(report.matched_detection_count, 1);
        assert!(report.detections[0].matched_iou.unwrap() < 0.50);
        assert_eq!(report.detections[0].detection_score, 1.0);
    }

    #[test]
    fn moderate_bounds_drift_gets_partial_detection_credit() {
        let spec = water_spec();
        let result = water_result("水", rect(20.0, 20.0, 30.0, 40.0));
        let report = score_eval(
            Path::new("image.png"),
            Path::new("eval.json"),
            &spec,
            &result,
            0.50,
        );
        assert_eq!(report.matched_detection_count, 1);
        assert!(report.detections[0].detection_score > 0.0);
        assert!(report.detections[0].detection_score < 1.0);
        assert!(!report.passed);
    }

    #[test]
    fn per_detection_bounds_tolerance_accepts_larger_jitter() {
        let mut spec = water_spec();
        spec.detections[0].bounds = rect(10.0, 20.0, 30.0, 10.0);
        spec.detections[0].bounds_tolerance = Some(BoundsTolerance {
            x: 4.0,
            y: 8.0,
            width: 6.0,
            height: 6.0,
        });
        spec.detections[0].characters[0].bounds = rect(10.0, 20.0, 30.0, 10.0);
        let result = water_result("水", rect(10.0, 28.0, 30.0, 10.0));
        let report = score_eval(
            Path::new("image.png"),
            Path::new("eval.json"),
            &spec,
            &result,
            0.50,
        );
        assert!(report.passed);
        assert_eq!(report.matched_detection_count, 1);
        assert_eq!(report.detections[0].detection_score, 1.0);
    }

    #[test]
    fn text_mismatch_reports_character_difference() {
        let spec = water_spec();
        let result = water_result("氷", rect(10.0, 20.0, 30.0, 40.0));
        let report = score_eval(
            Path::new("image.png"),
            Path::new("eval.json"),
            &spec,
            &result,
            0.50,
        );
        assert!(!report.passed);
        assert_eq!(report.detections[0].char_edit_distance, 1);
        assert_eq!(
            report.detections[0].character_differences[0].kind,
            CharacterDifferenceKind::Mismatch
        );
    }

    #[test]
    fn unmatched_detection_scores_zero_for_detection_and_text() {
        let spec = water_spec();
        let result = water_result("水", rect(500.0, 500.0, 30.0, 40.0));
        let report = score_eval(
            Path::new("image.png"),
            Path::new("eval.json"),
            &spec,
            &result,
            0.50,
        );
        assert_eq!(report.matched_detection_count, 0);
        assert_eq!(report.detections[0].detection_score, 0.0);
        assert_eq!(report.detections[0].character_score, 0.0);
    }

    #[test]
    fn unexpected_actual_detection_penalizes_scores() {
        let spec = water_spec();
        let result = two_line_result(
            "水",
            rect(10.0, 20.0, 30.0, 40.0),
            "extra",
            rect(100.0, 20.0, 50.0, 20.0),
        );
        let report = score_eval(
            Path::new("image.png"),
            Path::new("eval.json"),
            &spec,
            &result,
            0.50,
        );
        assert_eq!(report.unexpected_actual_count, 1);
        assert!(report.detection_score < 1.0);
        assert!(report.character_score < 1.0);
        assert!(report.metadata_score < 1.0);
    }

    #[test]
    fn ignored_actual_detection_does_not_penalize_scores() {
        let mut spec = water_spec();
        spec.ignored.push(IgnoredText {
            text: "extra".to_string(),
            reason: "background label".to_string(),
            bounds: Some(rect(100.0, 20.0, 50.0, 20.0)),
        });
        let result = two_line_result(
            "水",
            rect(10.0, 20.0, 30.0, 40.0),
            "extra",
            rect(100.0, 20.0, 50.0, 20.0),
        );
        let report = score_eval(
            Path::new("image.png"),
            Path::new("eval.json"),
            &spec,
            &result,
            0.50,
        );
        assert_eq!(report.ignored_actual_count, 1);
        assert_eq!(report.unexpected_actual_count, 0);
        assert!(report.passed);
    }

    #[test]
    fn discovers_eval_bundles_in_stable_order() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let second = root.join("b").join("capture-2");
        let first = root.join("a").join("capture-1");
        std::fs::create_dir_all(&second).unwrap();
        std::fs::create_dir_all(&first).unwrap();
        std::fs::write(second.join("eval.json"), "{}").unwrap();
        std::fs::write(second.join("underlying.png"), "not decoded by discovery").unwrap();
        std::fs::write(first.join("eval.json"), "{}").unwrap();
        std::fs::write(first.join("underlying.png"), "not decoded by discovery").unwrap();

        let bundles = discover_eval_bundles(root).unwrap();

        assert_eq!(bundles.len(), 2);
        assert!(bundles[0].labels.ends_with("a/capture-1/eval.json"));
        assert!(bundles[1].labels.ends_with("b/capture-2/eval.json"));
    }

    #[test]
    fn corpus_totals_weight_underlying_denominators() {
        let spec = water_spec();
        let perfect = score_eval(
            Path::new("image-1.png"),
            Path::new("eval-1.json"),
            &spec,
            &water_result("水", rect(10.0, 20.0, 30.0, 40.0)),
            0.50,
        );
        let with_unexpected = score_eval(
            Path::new("image-2.png"),
            Path::new("eval-2.json"),
            &spec,
            &two_line_result(
                "水",
                rect(10.0, 20.0, 30.0, 40.0),
                "extra",
                rect(100.0, 20.0, 50.0, 20.0),
            ),
            0.50,
        );

        let mut totals = CorpusScoreTotals::default();
        totals.add_report(&perfect);
        totals.add_report(&with_unexpected);

        assert!((totals.detection_score() - (2.0 / 3.0)).abs() < 0.0001);
        assert!((totals.character_score() - (2.0 / 7.0)).abs() < 0.0001);
        assert!((totals.metadata_score() - (2.0 / 3.0)).abs() < 0.0001);
    }
}
