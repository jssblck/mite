use std::borrow::Cow;
use std::collections::HashMap;
use std::fs;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use image::imageops::FilterType;
use image::{Rgb, RgbImage, imageops};
use ort::session::Session;
use ort::value::Tensor;
use rayon::prelude::*;

use crate::capture::Frame;
use crate::config::{ModelConfig, PipelineConfig, ResizeFilter, RuntimeConfig};
use crate::geometry::{Rect, Size};
use crate::ocr::{OcrEngine, RecognizedText, StableIdAllocator, TextBox};

mod providers;
mod text;

use providers::{ModelKind, commit_session, require_file};
use text::normalize_recognized_text;

/// Detector input channels (RGB).
const DET_CHANNELS: usize = 3;
/// ImageNet normalization applied to the detector input, per channel.
const DET_MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const DET_STD: [f32; 3] = [0.229, 0.224, 0.225];

/// Detector input dimensions are clamped up to at least this and rounded to a
/// multiple of it (PP-OCRv5 detector requires multiple-of-32 H/W).
const DET_SIZE_MULTIPLE: u32 = 32;
/// If stride-alignment padding would add more than this fraction to either
/// dimension, resize to the aligned shape instead. Large game frames only get a
/// narrow padded edge, while tiny eval/debug images avoid spending a meaningful
/// slice of the tensor on empty border.
const DET_ALIGNMENT_RESIZE_THRESHOLD: f32 = 0.05;

/// Detector TensorRT optimization-profile bounds (the dynamic H/W of the single
/// `NxCxHxW` input). The frame is downscaled only when it is above the native-4K
/// cap, so one engine built for this range serves the default high-recall 4K
/// path and the explicit low-resolution fast path; `opt` is the common
/// 16:9-at-1920 case.
const DET_PROFILE_MIN_SIDE: u32 = 64;
const DET_PROFILE_OPT_HEIGHT: u32 = 1088;
const DET_PROFILE_OPT_WIDTH: u32 = 1920;
/// Keep the profile exactly at native 4K: widening it (for example to cover
/// `detector_upscale` above 1.0) rebuilds the engine with different tactic
/// choices and measurably perturbs detection scores. Inputs beyond the
/// profile fall back to the CUDA execution provider with a logged warning.
const DET_PROFILE_MAX_SIDE: u32 = 3840;

/// Rec601 luma weights (R, G, B) for the grayscale/contrast-stretch helpers.
const LUMA_WEIGHTS: [f32; 3] = [0.299, 0.587, 0.114];

/// Robust luminance percentiles bounding the contrast stretch, so a few stray
/// pixels can't flatten it.
const CONTRAST_STRETCH_LOW_PCT: f32 = 0.02;
const CONTRAST_STRETCH_HIGH_PCT: f32 = 0.98;

/// Local-contrast radius, in detector-input pixels, used by the recall detector
/// pass. At native 4K, 14 px covers the neighborhood around typical game UI
/// glyph strokes without swallowing whole text lines.
const LOCAL_CONTRAST_RADIUS_PX: u32 = 14;
/// Gain for the high-pass luminance image used by the recall detector pass.
const LOCAL_CONTRAST_GAIN: f32 = 6.0;

/// Minimum side and area (in detector-map pixels) for a detected component to be
/// treated as plausible text; tiny specks are rejected.
const MIN_TEXT_RECT_SIDE_PX: f32 = 8.0;
const MIN_TEXT_RECT_AREA_PX: f32 = 16.0;
/// Overlap thresholds used to dedupe boxes discovered by the normal and
/// low-contrast detector passes.
const DETECTION_DEDUPE_IOU: f32 = 0.35;
const DETECTION_DEDUPE_CONTAINMENT: f32 = 0.70;

/// Quantization bucket (px) for assigning cross-frame-stable detection box ids:
/// boxes whose rect rounds to the same bucket keep their id.
const STABLE_ID_BUCKET_PX: f32 = 6.0;

/// Time `f` and print to stderr when `MITE_PROFILE` is set in the environment.
/// Zero overhead (one env lookup) when disabled; used to break the detect/
/// recognize stages into sub-steps so we can see where the GPU pipeline's time
/// actually goes (CPU preprocessing vs. inference vs. postprocessing).
#[inline]
fn prof<T>(label: &str, f: impl FnOnce() -> T) -> T {
    if std::env::var_os("MITE_PROFILE").is_some() {
        let start = Instant::now();
        let out = f();
        eprintln!(
            "[prof] {label}: {:.2}ms",
            start.elapsed().as_secs_f64() * 1000.0
        );
        out
    } else {
        f()
    }
}

/// Lines the primary recognizer reads below this confidence get a second
/// opinion from the optional fallback recognizer.
const REC_FALLBACK_MAX_CONFIDENCE: f32 = 0.75;
/// The fallback's read replaces the primary's only at or above this absolute
/// confidence (and only when it also beats the primary's confidence).
const REC_FALLBACK_ACCEPT_MIN: f32 = 0.92;

/// Maximum text lines per recognizer batch. Recognizer compute scales with
/// `batch × padded_width`, and every line in a batch is zero-padded to the
/// batch's widest member — so a few moderate, width-sorted batches (each spanning
/// a narrow width range) beat one giant batch that pads short lines out to the
/// longest line's width. The small per-call overhead (~4 ms) is cheap next to the
/// padding compute it saves.
const REC_BATCH_MAX: usize = 12;

/// Recognizer input height in pixels (PP-OCRv5 rec models are trained at 48px).
const REC_INPUT_HEIGHT: u32 = 48;
/// Recognizer input channels (RGB).
const REC_CHANNELS: usize = 3;
/// Recognizer crop width is clamped into this range, then rounded up to a
/// multiple of [`REC_WIDTH_MULTIPLE`]. The TensorRT optimization profile is
/// derived from these bounds (see [`trt_profile_shapes`]) so they can't drift.
const REC_MIN_WIDTH: u32 = 16;
const REC_MAX_WIDTH: u32 = 960;
/// Mid-range width the TensorRT profile is optimized for.
const REC_OPT_WIDTH: u32 = 320;
/// Recognizer tensor width is rounded up to a multiple of this.
const REC_WIDTH_MULTIPLE: u32 = 8;

/// A preprocessed text-line crop awaiting batched recognition.
struct RecCrop {
    /// Position in the caller's `boxes`, so results land back in order.
    index: usize,
    /// Resized line image (height 48, width `real_w`).
    resized: RgbImage,
    /// Width of `resized` before any batch padding.
    real_w: usize,
    /// Left edge and width of the padded crop region in frame pixels, for mapping
    /// per-glyph CTC positions back onto the image.
    crop_x: u32,
    crop_w: u32,
}

struct DetectorInput<'a> {
    image: Cow<'a, RgbImage>,
    /// Tensor shape after padding to PP-OCR's multiple-of-32 input size.
    padded_size: Size,
}

impl DetectorInput<'_> {
    fn content_size(&self) -> Size {
        Size::new(self.image.width(), self.image.height())
    }
}

pub struct OrtOcrEngine {
    detector: Session,
    recognizer: Session,
    /// Optional heavier second-opinion recognizer for low-confidence lines.
    fallback_recognizer: Option<Session>,
    dictionary: Vec<String>,
    stable_ids: StableIdAllocator,
    /// Set from the pipeline config during `detect`, reused by `recognize`
    /// (which has no config argument) so both stages see the same preprocessing.
    contrast_stretch: bool,
    /// The recognition-confidence floor from the pipeline config, captured in
    /// `detect`; lines below it never receive a fallback read (junk must stay
    /// dead rather than having its confidence boosted past the noise filters).
    min_recognition_confidence: f32,
}

impl OrtOcrEngine {
    pub fn new(models: &ModelConfig, runtime: &RuntimeConfig) -> Result<Self> {
        let resolve_int8 = |enabled: bool, path: &std::path::Path| -> Result<std::path::PathBuf> {
            if !enabled {
                return Ok(path.to_path_buf());
            }
            let int8 = crate::config::int8_model_path(path);
            if !int8.exists() {
                bail!(
                    "INT8 is enabled but {} is missing; generate the INT8 models \
                     with `.venv-models\\Scripts\\python.exe scripts\\quantize-models.py`",
                    int8.display()
                );
            }
            Ok(int8)
        };
        let detector_path = resolve_int8(runtime.int8_detector, &models.detector_path)?;
        let recognizer_path = resolve_int8(runtime.int8_recognizer, &models.recognizer_path)?;
        require_file(&detector_path)?;
        require_file(&recognizer_path)?;
        let charset_path = models
            .charset_path
            .as_ref()
            .context("models.charset_path is required for real OCR")?;
        require_file(charset_path)?;

        let detector = commit_session(&detector_path, runtime, ModelKind::Detector)?;
        let recognizer = commit_session(&recognizer_path, runtime, ModelKind::Recognizer)?;
        let fallback_recognizer = match &models.fallback_recognizer_path {
            Some(path) => {
                require_file(path)?;
                // The PP-OCRv5 server recognizer overflows FP16 (see
                // docs/models.md); the fallback always runs FP32, never INT8.
                let fp32_runtime = RuntimeConfig {
                    fp16: false,
                    int8_recognizer: false,
                    ..runtime.clone()
                };
                Some(commit_session(path, &fp32_runtime, ModelKind::Recognizer)?)
            }
            None => None,
        };
        let dictionary = fs::read_to_string(charset_path)
            .with_context(|| format!("failed to read {}", charset_path.display()))?
            .lines()
            .map(str::to_string)
            .collect::<Vec<_>>();

        Ok(Self {
            detector,
            recognizer,
            fallback_recognizer,
            dictionary,
            stable_ids: StableIdAllocator::default(),
            contrast_stretch: false,
            min_recognition_confidence: 0.5,
        })
    }
}

impl OcrEngine for OrtOcrEngine {
    fn detect(&mut self, frame: &Frame, config: &PipelineConfig) -> Result<Vec<TextBox>> {
        let image = frame
            .pixels
            .as_ref()
            .context("real OCR requires frames with RGB pixels")?;
        self.contrast_stretch = config.detector_contrast_stretch;
        self.min_recognition_confidence = config.min_recognition_confidence;
        let image = maybe_contrast_stretch(image, self.contrast_stretch);
        let native_long = image.width().max(image.height());
        let target_long = config.detector_target_long_side(native_long);
        let detector_input = prof("det.resize", || {
            resize_for_detector(image.as_ref(), target_long, config.detector_resize_filter)
        });
        // Dual-pass scheduling: the GPU should never wait for CPU work between
        // the two detector inferences. While the primary inference holds the
        // session, a worker thread builds the local-contrast image *and* its
        // NCHW tensor; the moment the primary inference returns, the
        // low-contrast inference launches and the primary probability map's
        // postprocessing runs concurrently on a second thread.
        let candidates = if config.detector_low_contrast_pass {
            let primary_cfg = DetectorPassConfig::primary(config);
            let low_cfg = DetectorPassConfig::low_contrast(config);
            let padded_size = detector_input.padded_size;
            let content_size = detector_input.content_size();
            let frame_size = frame.size;
            std::thread::scope(|scope| -> Result<Vec<ScoredRect>> {
                let low_input_thread = scope.spawn(|| {
                    let enhanced = prof("det.local_contrast", || {
                        local_contrast_detector_image(detector_input.image.as_ref())
                    });
                    let enhanced_input = DetectorInput {
                        image: Cow::Owned(enhanced),
                        padded_size,
                    };
                    let tensor = prof("det.low_contrast.tensor", || {
                        detector_tensor(&enhanced_input)
                    });
                    (enhanced_input, tensor)
                });
                let primary_tensor =
                    prof("det.primary.tensor", || detector_tensor(&detector_input));
                let (primary_value, map_w, map_h) = detector_probability_value(
                    &mut self.detector,
                    primary_tensor,
                    padded_size,
                    "det.primary",
                )?;
                let (enhanced_input, low_tensor) = low_input_thread
                    .join()
                    .map_err(|_| anyhow::anyhow!("local contrast build panicked"))?;
                let primary_components = scope.spawn(move || -> Result<Vec<ScoredRect>> {
                    let (_, primary_map) = primary_value.try_extract_tensor::<f32>()?;
                    Ok(prof("det.primary.components", || {
                        candidates_from_probability_map(
                            ProbabilityMap {
                                data: primary_map,
                                width: map_w,
                                height: map_h,
                            },
                            DetectorPostprocess {
                                input_size: padded_size,
                                content_size,
                                frame_size,
                                config,
                                pass_config: primary_cfg,
                            },
                        )
                    }))
                });
                let low = detect_candidate_rects_with_tensor(
                    &mut self.detector,
                    low_tensor,
                    &enhanced_input,
                    frame_size,
                    config,
                    low_cfg,
                )?;
                let mut candidates = primary_components
                    .join()
                    .map_err(|_| anyhow::anyhow!("primary postprocessing panicked"))??;
                candidates.extend(low);
                Ok(candidates)
            })?
        } else {
            prof("det.primary", || {
                detect_candidate_rects(
                    &mut self.detector,
                    &detector_input,
                    frame.size,
                    config,
                    DetectorPassConfig::primary(config),
                )
            })?
        };

        let boxes = finalize_detection_boxes(
            candidates,
            frame.size,
            config,
            &mut self.stable_ids,
            frame.content_epoch,
        );

        Ok(boxes)
    }

    fn recognize(&mut self, frame: &Frame, boxes: &[TextBox]) -> Result<Vec<RecognizedText>> {
        let image = frame
            .pixels
            .as_ref()
            .context("real OCR requires frames with RGB pixels")?;
        let image = maybe_contrast_stretch(image, self.contrast_stretch);
        if boxes.is_empty() {
            return Ok(Vec::new());
        }

        // Preprocess each box once (crop + resize to height 48). Per-call GPU
        // launch/transfer overhead dominates a single tiny line, so instead of one
        // `recognizer.run` per box we group boxes of similar width into batches and
        // run a handful of times. Widths vary per line, so each batch is padded to
        // its widest member (`target_w`); grouping keeps that padding small.
        // Crop + resize every line in parallel — these are independent and the
        // frame is read-only, so this scales across cores (the loop was a large
        // slice of the recognize stage on busy frames).
        let frame_image = image.as_ref();
        let crops: Vec<RecCrop> = prof("rec.crop_resize", || {
            boxes
                .par_iter()
                .enumerate()
                .map(|(index, text_box)| -> Result<RecCrop> {
                    let crop = crop_text_line(frame_image, text_box.rect);
                    let resized = resize_for_recognizer(&crop);
                    let bounds =
                        crop_bounds(frame_image.width(), frame_image.height(), text_box.rect);
                    Ok(RecCrop {
                        index,
                        real_w: resized.width() as usize,
                        resized,
                        crop_x: bounds.x,
                        crop_w: bounds.width,
                    })
                })
                .collect::<Result<Vec<_>>>()
        })?;

        // Order lines by width, then pack into as few batches as possible. The
        // recognizer's per-call overhead dominates its per-line compute, so the
        // win is minimizing call count; sorting by width first keeps each batch's
        // zero-padding (to its widest member) small despite the larger batches.
        let mut order: Vec<usize> = (0..crops.len()).collect();
        order.sort_by_key(|&index| crops[index].real_w);

        let mut results: Vec<Option<RecognizedText>> = (0..boxes.len()).map(|_| None).collect();
        prof("rec.batches", || {
            run_recognizer_batches_pipelined(
                &mut self.recognizer,
                &self.dictionary,
                &crops,
                &order,
                boxes,
                &mut results,
            )
        })?;

        // Second opinion from the heavier fallback recognizer for lines the
        // primary read with low confidence. Crops are reused as-is; both
        // recognizers share the input shape, normalization, and charset. The
        // fallback's read wins only when it clears a high absolute confidence
        // bar and beats the primary, so cross-model confidence noise cannot
        // churn ordinary lines, and lines below the recognition floor are
        // never upgraded.
        if let Some(fallback) = self.fallback_recognizer.as_mut() {
            let floor = self.min_recognition_confidence;
            let consult: Vec<usize> = results
                .iter()
                .enumerate()
                .filter(|(_, slot)| {
                    slot.as_ref().is_some_and(|line| {
                        !line.text.trim().is_empty()
                            && line.confidence >= floor
                            && line.confidence < REC_FALLBACK_MAX_CONFIDENCE
                    })
                })
                .map(|(index, _)| index)
                .collect();
            if !consult.is_empty() {
                prof("rec.fallback", || -> Result<()> {
                    let mut fallback_results: Vec<Option<RecognizedText>> =
                        (0..boxes.len()).map(|_| None).collect();
                    let mut consult_order: Vec<usize> = Vec::new();
                    for (crop_index, crop) in crops.iter().enumerate() {
                        if consult.contains(&crop.index) {
                            consult_order.push(crop_index);
                        }
                    }
                    consult_order.sort_by_key(|&crop_index| crops[crop_index].real_w);
                    for chunk in consult_order.chunks(REC_BATCH_MAX) {
                        run_recognizer_batch(
                            fallback,
                            &self.dictionary,
                            &crops,
                            chunk,
                            boxes,
                            &mut fallback_results,
                        )?;
                    }
                    for index in consult {
                        if let (Some(original), Some(second)) =
                            (results[index].as_ref(), fallback_results[index].take())
                            && second.confidence >= REC_FALLBACK_ACCEPT_MIN
                            && second.confidence > original.confidence
                            && !second.text.trim().is_empty()
                        {
                            results[index] = Some(second);
                        }
                    }
                    Ok(())
                })?;
            }
        }

        prof("rec.shape_rescue", || {
            rescue_shape_glyphs(frame_image, &mut results)
        });

        let mut recognized = Vec::with_capacity(results.len());
        for (index, result) in results.into_iter().enumerate() {
            recognized.push(
                result.with_context(|| format!("recognizer produced no result for box {index}"))?,
            );
        }
        Ok(recognized)
    }
}

/// Diamond list bullets (◇/◆) are real, labeled UI glyphs in the target games,
/// but they are not in the recognizer charset, so their detected boxes decode
/// to empty text and get dropped as noise. For small near-square boxes that
/// decoded empty, classify the glyph geometrically on the frame and synthesize
/// the recognition, with the box refined to the measured glyph extent (the
/// detector typically boxes only the brighter inner part of the diamond).
fn rescue_shape_glyphs(image: &RgbImage, results: &mut [Option<RecognizedText>]) {
    let text_rects: Vec<Rect> = results
        .iter()
        .flatten()
        .filter(|line| !line.text.trim().is_empty())
        .map(|line| line.text_box.rect)
        .collect();

    for slot in results.iter_mut().flatten() {
        if !slot.text.trim().is_empty() {
            continue;
        }
        let rect = slot.text_box.rect;
        if !(12.0..=90.0).contains(&rect.height) {
            continue;
        }
        let aspect = rect.width / rect.height.max(1.0);
        if !(0.5..=1.8).contains(&aspect) {
            continue;
        }
        // A bullet introduces text on the same row; lone diamonds are timeline
        // markers and map decorations, not glyphs.
        if !has_same_row_text(rect, &text_rects) {
            continue;
        }
        if let Some((glyph, measured)) = classify_diamond_glyph(image, rect) {
            slot.text = glyph.to_string();
            slot.confidence = 0.95;
            slot.text_box.rect = measured;
            slot.char_centers = vec![measured.x + measured.width / 2.0];
        }
    }
}

/// Whether any recognized text line shares this rect's row (>= half-height
/// vertical overlap) within a few glyph widths horizontally.
fn has_same_row_text(rect: Rect, text_rects: &[Rect]) -> bool {
    let max_gap = rect.height * 4.0;
    text_rects.iter().any(|other| {
        let overlap = (rect.bottom().min(other.bottom()) - rect.y.max(other.y)).max(0.0);
        if overlap < rect.height * 0.5 {
            return false;
        }
        let gap = if other.x >= rect.right() {
            other.x - rect.right()
        } else if rect.x >= other.right() {
            rect.x - other.right()
        } else {
            0.0
        };
        gap <= max_gap
    })
}

/// Decide whether the region around `rect` holds a diamond bullet. Returns the
/// glyph (outline `◇` or filled `◆`) and the measured glyph bounding box in
/// frame coordinates.
fn classify_diamond_glyph(image: &RgbImage, rect: Rect) -> Option<(char, Rect)> {
    // Probe a neighborhood larger than the detector box: the box usually covers
    // only the glyph's bright core.
    let margin = rect.height * 0.75;
    let x0 = (rect.x - margin).max(0.0) as u32;
    let y0 = (rect.y - margin).max(0.0) as u32;
    let x1 = ((rect.right() + margin) as u32).min(image.width().saturating_sub(1));
    let y1 = ((rect.bottom() + margin) as u32).min(image.height().saturating_sub(1));
    if x1 <= x0 + 8 || y1 <= y0 + 8 {
        return None;
    }

    let width = (x1 - x0 + 1) as usize;
    let height = (y1 - y0 + 1) as usize;
    let mut luma = vec![0f32; width * height];
    for y in 0..height {
        for x in 0..width {
            let pixel = image.get_pixel(x0 + x as u32, y0 + y as u32);
            luma[y * width + x] = LUMA_WEIGHTS[0] * pixel[0] as f32
                + LUMA_WEIGHTS[1] * pixel[1] as f32
                + LUMA_WEIGHTS[2] * pixel[2] as f32;
        }
    }

    // Background reference: median of the probe border ring.
    let mut border: Vec<f32> = Vec::with_capacity(2 * (width + height));
    for x in 0..width {
        border.push(luma[x]);
        border.push(luma[(height - 1) * width + x]);
    }
    for y in 0..height {
        border.push(luma[y * width]);
        border.push(luma[y * width + width - 1]);
    }
    border.sort_by(f32::total_cmp);
    let background = border[border.len() / 2];

    const FG_LUMA_DELTA: f32 = 45.0;
    let foreground: Vec<bool> = luma
        .iter()
        .map(|&value| (value - background).abs() > FG_LUMA_DELTA)
        .collect();

    // Text-layer glyphs are sharp; out-of-focus UI chrome (blurred buttons
    // behind a modal) smears into shapes that pass soft geometry tests. Demand
    // at least one crisp edge anywhere in the probe.
    const MIN_PEAK_EDGE_GRADIENT: f32 = 55.0;
    let mut peak_gradient = 0f32;
    for y in 0..height {
        for x in 0..width.saturating_sub(1) {
            peak_gradient =
                peak_gradient.max((luma[y * width + x + 1] - luma[y * width + x]).abs());
        }
    }
    for y in 0..height.saturating_sub(1) {
        for x in 0..width {
            peak_gradient =
                peak_gradient.max((luma[(y + 1) * width + x] - luma[y * width + x]).abs());
        }
    }
    if peak_gradient < MIN_PEAK_EDGE_GRADIENT {
        return None;
    }

    let mut min_x = usize::MAX;
    let mut max_x = 0usize;
    let mut min_y = usize::MAX;
    let mut max_y = 0usize;
    let mut count = 0usize;
    let mut sum_x = 0f32;
    let mut sum_y = 0f32;
    for y in 0..height {
        for x in 0..width {
            if foreground[y * width + x] {
                count += 1;
                sum_x += x as f32;
                sum_y += y as f32;
                min_x = min_x.min(x);
                max_x = max_x.max(x);
                min_y = min_y.min(y);
                max_y = max_y.max(y);
            }
        }
    }
    if count < 24 || max_x <= min_x + 9 || max_y <= min_y + 9 {
        return None;
    }
    let glyph_w = (max_x - min_x + 1) as f32;
    let glyph_h = (max_y - min_y + 1) as f32;
    let glyph_aspect = glyph_w / glyph_h;
    if !(0.7..=1.4).contains(&glyph_aspect) {
        return None;
    }
    // Labeled diamond bullets are 17-42 px; larger rhombic shapes are game
    // decorations, not text glyphs.
    if !(12.0..=50.0).contains(&glyph_w) || !(12.0..=50.0).contains(&glyph_h) {
        return None;
    }

    let cx = sum_x / count as f32;
    let cy = sum_y / count as f32;
    let rx = (glyph_w / 2.0).max(1.0);
    let ry = (glyph_h / 2.0).max(1.0);

    // A diamond's mass lies inside the L1 ball spanned by its half-extents and
    // its bounding-box corners are empty; reject squares, letters, and icons.
    let mut inside = 0usize;
    let mut corner_total = 0usize;
    let mut corner_foreground = 0usize;
    let mut quadrant = [false; 4];
    let mut core_total = 0usize;
    let mut core_foreground = 0usize;
    let mut ring_total = 0usize;
    let mut ring_foreground = 0usize;
    // Glyph mass near each axis-extreme vertex (top, bottom, left, right). A
    // diamond peaks exactly there; bracket/reticle icons have gaps.
    let mut vertex_foreground = [0usize; 4];
    // Glyph mass near each 45-degree edge midpoint. A diamond's outline runs
    // exactly there; circular reticle icons with diagonal notches do not.
    let mut edge_foreground = [0usize; 4];
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let dx = (x as f32 - cx) / rx;
            let dy = (y as f32 - cy) / ry;
            let l1 = dx.abs() + dy.abs();
            let is_fg = foreground[y * width + x];
            if is_fg {
                if dy <= -0.7 && dx.abs() <= 0.3 {
                    vertex_foreground[0] += 1;
                }
                if dy >= 0.7 && dx.abs() <= 0.3 {
                    vertex_foreground[1] += 1;
                }
                if dx <= -0.7 && dy.abs() <= 0.3 {
                    vertex_foreground[2] += 1;
                }
                if dx >= 0.7 && dy.abs() <= 0.3 {
                    vertex_foreground[3] += 1;
                }
                if (dx.abs() - 0.5).abs() <= 0.22 && (dy.abs() - 0.5).abs() <= 0.22 {
                    let qx = usize::from(dx >= 0.0);
                    let qy = usize::from(dy >= 0.0);
                    edge_foreground[qy * 2 + qx] += 1;
                }
            }
            if l1 > 1.3 {
                corner_total += 1;
                if is_fg {
                    corner_foreground += 1;
                }
            }
            if l1 <= 0.40 {
                core_total += 1;
                if is_fg {
                    core_foreground += 1;
                }
            } else if (0.45..=0.75).contains(&l1) {
                ring_total += 1;
                if is_fg {
                    ring_foreground += 1;
                }
            }
            if is_fg {
                if l1 <= 1.2 {
                    inside += 1;
                }
                let qx = usize::from(dx >= 0.0);
                let qy = usize::from(dy >= 0.0);
                quadrant[qy * 2 + qx] = true;
            }
        }
    }

    if (inside as f32) < count as f32 * 0.92 {
        return None;
    }
    if corner_total > 0 && corner_foreground as f32 > corner_total as f32 * 0.08 {
        return None;
    }
    if quadrant.iter().any(|present| !present) {
        return None;
    }
    if vertex_foreground.iter().any(|&count| count < 2) {
        return None;
    }
    if edge_foreground.iter().any(|&count| count < 2) {
        return None;
    }

    // A true diamond's horizontal extent tapers linearly toward the vertical
    // tips; rhombic-ish icon blobs do not.
    let mut taper_error = 0f32;
    let mut taper_rows = 0usize;
    for y in min_y..=max_y {
        let dy = ((y as f32 - cy) / ry).abs();
        if dy > 0.9 {
            continue;
        }
        let mut row_min = None;
        let mut row_max = None;
        for x in min_x..=max_x {
            if foreground[y * width + x] {
                row_min.get_or_insert(x);
                row_max = Some(x);
            }
        }
        taper_rows += 1;
        match row_min.zip(row_max) {
            Some((row_left, row_right)) => {
                let actual_half = (row_right - row_left + 1) as f32 / 2.0 / rx;
                taper_error += (actual_half - (1.0 - dy)).abs();
            }
            None => taper_error += 1.0,
        }
    }
    if taper_rows == 0 || taper_error / taper_rows as f32 > 0.18 {
        return None;
    }

    let core_fill = if core_total == 0 {
        0.0
    } else {
        core_foreground as f32 / core_total as f32
    };
    let ring_fill = if ring_total == 0 {
        0.0
    } else {
        ring_foreground as f32 / ring_total as f32
    };
    // Filled core with an empty ring is a nested glyph (e.g. the gold ◈ event
    // bullet), which is neither ◇ nor ◆; do not synthesize a wrong glyph.
    if core_fill >= 0.55 && ring_fill <= 0.35 {
        return None;
    }
    let glyph = if core_fill >= 0.5 { '◆' } else { '◇' };
    let measured = Rect::new(
        x0 as f32 + min_x as f32,
        y0 as f32 + min_y as f32,
        glyph_w,
        glyph_h,
    );
    Some((glyph, measured))
}

/// Optionally apply a per-image luminance contrast stretch. Returns the input
/// unchanged (borrowed) when disabled, so the common path costs nothing.
fn maybe_contrast_stretch(image: &RgbImage, enabled: bool) -> Cow<'_, RgbImage> {
    if enabled {
        Cow::Owned(contrast_stretch(image))
    } else {
        Cow::Borrowed(image)
    }
}

/// Linear min/max contrast stretch driven by robust luminance percentiles
/// (2nd/98th), mapping that range onto 0..=255. This rescues very low-contrast
/// text (e.g. grey on grey) that the detector otherwise scores as background.
/// Uses the 2nd/98th percentiles rather than raw min/max so a few stray pixels
/// can't flatten the stretch.
fn contrast_stretch(image: &RgbImage) -> RgbImage {
    let mut histogram = [0u32; 256];
    for pixel in image.pixels() {
        histogram[luminance(pixel) as usize] += 1;
    }
    let total: u32 = histogram.iter().sum();
    if total == 0 {
        return image.clone();
    }

    let lo = percentile_bin(&histogram, total, CONTRAST_STRETCH_LOW_PCT);
    let hi = percentile_bin(&histogram, total, CONTRAST_STRETCH_HIGH_PCT);
    if hi <= lo {
        return image.clone();
    }

    let scale = 255.0 / (hi as f32 - lo as f32);
    let mut out = image.clone();
    for pixel in out.pixels_mut() {
        for channel in 0..3 {
            let stretched = (pixel[channel] as f32 - lo as f32) * scale;
            pixel[channel] = stretched.clamp(0.0, 255.0) as u8;
        }
    }
    out
}

fn luminance(pixel: &Rgb<u8>) -> u8 {
    let [r, g, b] = pixel.0;
    (LUMA_WEIGHTS[0] * r as f32 + LUMA_WEIGHTS[1] * g as f32 + LUMA_WEIGHTS[2] * b as f32).round()
        as u8
}

/// Smallest luminance bin at or below which `fraction` of pixels fall.
fn percentile_bin(histogram: &[u32; 256], total: u32, fraction: f32) -> u8 {
    let target = (total as f32 * fraction) as u32;
    let mut cumulative = 0u32;
    for (bin, count) in histogram.iter().enumerate() {
        cumulative += count;
        if cumulative >= target {
            return bin as u8;
        }
    }
    255
}

/// Build a detector-only local-contrast view. Recognition still crops from the
/// original frame; this image exists only to let the detector see faint glyph
/// strokes on translucent panels and grey-on-grey game UI. This build runs
/// concurrently with the primary detector inference, so most of its cost stays
/// off the latency critical path.
fn local_contrast_detector_image(image: &RgbImage) -> RgbImage {
    let width = image.width() as usize;
    let height = image.height() as usize;
    if width == 0 || height == 0 {
        return image.clone();
    }
    let radius = LOCAL_CONTRAST_RADIUS_PX as usize;

    // Separable sliding-window box mean instead of a u64 integral image: the
    // integral build was single-threaded and its (w+1)x(h+1) u64 buffer cost
    // ~66 MB of allocation and traffic at 4K. Every accumulation below is exact
    // integer math over the same clamped window, so the output is bit-identical
    // to the integral formulation.
    let raw = image.as_raw();
    let mut luma = vec![0u8; width * height];
    luma.par_chunks_mut(width)
        .zip(raw.par_chunks(width * 3))
        .for_each(|(dst, src)| {
            for (out, px) in dst.iter_mut().zip(src.chunks_exact(3)) {
                *out = luminance(&Rgb([px[0], px[1], px[2]]));
            }
        });

    // Horizontal clamped moving sums, rows independent. Window max is
    // (2*radius+1) * 255, far inside u32.
    let mut hsum = vec![0u32; width * height];
    hsum.par_chunks_mut(width)
        .zip(luma.par_chunks(width))
        .for_each(|(dst, row)| {
            let mut sum: u32 = row[..(radius + 1).min(width)]
                .iter()
                .map(|&v| v as u32)
                .sum();
            dst[0] = sum;
            for x in 1..width {
                if x + radius < width {
                    sum += row[x + radius] as u32;
                }
                if x > radius {
                    sum -= row[x - radius - 1] as u32;
                }
                dst[x] = sum;
            }
        });

    let hcount: Vec<u32> = (0..width)
        .map(|x| ((x + radius + 1).min(width) - x.saturating_sub(radius)) as u32)
        .collect();

    // Vertical pass fused with output: parallel over contiguous row chunks, each
    // chunk seeding its vertical window once and then sliding it row by row.
    // The seed costs window-height row additions per chunk, negligible next to
    // the per-row work it saves.
    let chunk_rows = height
        .div_ceil((rayon::current_num_threads() * 4).max(1))
        .max(1);
    let mut out = vec![0u8; width * height * 3];
    out.par_chunks_mut(chunk_rows * width * 3)
        .enumerate()
        .for_each(|(chunk, out_rows)| {
            let y_start = chunk * chunk_rows;
            let y_end = (y_start + chunk_rows).min(height);
            let mut vsum = vec![0u32; width];
            for r in y_start.saturating_sub(radius)..(y_start + radius + 1).min(height) {
                let row = &hsum[r * width..(r + 1) * width];
                for (acc, &value) in vsum.iter_mut().zip(row) {
                    *acc += value;
                }
            }
            for (y, out_row) in (y_start..y_end).zip(out_rows.chunks_mut(width * 3)) {
                let vcount = ((y + radius + 1).min(height) - y.saturating_sub(radius)) as u32;
                let luma_row = &luma[y * width..(y + 1) * width];
                for x in 0..width {
                    let local_mean = vsum[x] as f32 / (hcount[x] * vcount) as f32;
                    let value = 128.0 + (luma_row[x] as f32 - local_mean) * LOCAL_CONTRAST_GAIN;
                    let value = value.round().clamp(0.0, 255.0) as u8;
                    let offset = x * 3;
                    out_row[offset] = value;
                    out_row[offset + 1] = value;
                    out_row[offset + 2] = value;
                }
                if y + 1 < y_end {
                    if y + radius + 1 < height {
                        let row = &hsum[(y + radius + 1) * width..(y + radius + 2) * width];
                        for (acc, &value) in vsum.iter_mut().zip(row) {
                            *acc += value;
                        }
                    }
                    if y + 1 > radius {
                        let row = &hsum[(y - radius) * width..(y - radius + 1) * width];
                        for (acc, &value) in vsum.iter_mut().zip(row) {
                            *acc -= value;
                        }
                    }
                }
            }
        });

    RgbImage::from_raw(image.width(), image.height(), out)
        .expect("local contrast buffer matches dimensions")
}

fn resize_for_detector(
    image: &RgbImage,
    long_side: u32,
    filter: ResizeFilter,
) -> DetectorInput<'_> {
    let source_size = crate::geometry::Size::new(image.width(), image.height());
    let base_size =
        crate::geometry::Size::new(image.width(), image.height()).scaled_to_long_side(long_side);
    let padded_w = base_size
        .width
        .max(DET_SIZE_MULTIPLE)
        .next_multiple_of(DET_SIZE_MULTIPLE);
    let padded_h = base_size
        .height
        .max(DET_SIZE_MULTIPLE)
        .next_multiple_of(DET_SIZE_MULTIPLE);
    let padded_size = Size::new(padded_w, padded_h);
    let resize_for_alignment = detector_alignment_padding_is_material(base_size, padded_size);
    let resize_target = if resize_for_alignment {
        padded_size
    } else {
        base_size
    };

    // Downscaling a 4K game frame with a single-threaded bicubic filter was the
    // single largest cost in the whole pipeline (~200 ms). When *both* axes
    // shrink — the common case for 4K/1440p targets — use a rayon-parallel
    // area-averaging resampler instead: each output pixel is the coverage-
    // weighted mean of the source pixels it spans. For downscaling that is both
    // faster (parallel, cache-friendly, no per-pixel transcendental weights) and
    // higher quality than bicubic (it integrates all source detail and can't ring
    // or alias), so fine text strokes survive the shrink. Stride alignment is
    // usually tensor padding after resize; only tiny or unusually shaped inputs
    // resize to the aligned dimensions because the empty border would otherwise
    // be a material part of the detector tensor.
    let resized: Cow<'_, RgbImage> = if resize_target == source_size {
        Cow::Borrowed(image)
    } else if resize_target.width <= image.width() && resize_target.height <= image.height() {
        Cow::Owned(area_downscale(
            image,
            resize_target.width,
            resize_target.height,
        ))
    } else {
        Cow::Owned(imageops::resize(
            image,
            resize_target.width,
            resize_target.height,
            filter_type(filter),
        ))
    };

    DetectorInput {
        image: resized,
        padded_size,
    }
}

fn detector_alignment_padding_is_material(content: Size, padded: Size) -> bool {
    fn extra_fraction(content: u32, padded: u32) -> f32 {
        if content == 0 {
            return 0.0;
        }
        padded.saturating_sub(content) as f32 / content as f32
    }

    extra_fraction(content.width, padded.width) > DET_ALIGNMENT_RESIZE_THRESHOLD
        || extra_fraction(content.height, padded.height) > DET_ALIGNMENT_RESIZE_THRESHOLD
}

/// Per-output-sample source footprint along one axis: the first source index and
/// the normalized coverage weights for the contiguous source samples it spans.
fn axis_weights(src_len: u32, dst_len: u32) -> Vec<(u32, Vec<f32>)> {
    let scale = src_len as f32 / dst_len as f32;
    (0..dst_len)
        .map(|d| {
            let start_f = d as f32 * scale;
            let end_f = (d as f32 + 1.0) * scale;
            let start = start_f.floor() as u32;
            let end = (end_f.ceil() as u32).min(src_len).max(start + 1);
            let mut weights = Vec::with_capacity((end - start) as usize);
            let mut total = 0.0f32;
            for s in start..end {
                let lo = (s as f32).max(start_f);
                let hi = ((s + 1) as f32).min(end_f);
                let w = (hi - lo).max(0.0);
                weights.push(w);
                total += w;
            }
            if total > 0.0 {
                for w in &mut weights {
                    *w /= total;
                }
            }
            (start, weights)
        })
        .collect()
}

/// Coverage-weighted area downscale (separable: horizontal then vertical), with
/// both passes parallelized across rows with rayon. High quality for shrinking
/// and far cheaper than a single-threaded bicubic pass.
fn area_downscale(src: &RgbImage, dst_w: u32, dst_h: u32) -> RgbImage {
    let (sw, sh) = (src.width(), src.height());
    let src_buf = src.as_raw();

    // Horizontal pass: sw×sh → dst_w×sh into an f32 intermediate.
    let col_taps = axis_weights(sw, dst_w);
    let mut horiz = vec![0.0f32; dst_w as usize * sh as usize * 3];
    horiz
        .par_chunks_mut(dst_w as usize * 3)
        .enumerate()
        .for_each(|(y, out_row)| {
            let row_base = y * sw as usize * 3;
            for (dx, (start, weights)) in col_taps.iter().enumerate() {
                let (mut r, mut g, mut b) = (0.0f32, 0.0f32, 0.0f32);
                let mut si = row_base + *start as usize * 3;
                for &w in weights {
                    r += src_buf[si] as f32 * w;
                    g += src_buf[si + 1] as f32 * w;
                    b += src_buf[si + 2] as f32 * w;
                    si += 3;
                }
                let o = dx * 3;
                out_row[o] = r;
                out_row[o + 1] = g;
                out_row[o + 2] = b;
            }
        });

    // Vertical pass: dst_w×sh → dst_w×dst_h, writing the final u8 image.
    let row_taps = axis_weights(sh, dst_h);
    let mut out = vec![0u8; dst_w as usize * dst_h as usize * 3];
    let stride = dst_w as usize * 3;
    out.par_chunks_mut(stride)
        .enumerate()
        .for_each(|(dy, out_row)| {
            let (start, weights) = &row_taps[dy];
            let base = *start as usize * stride;
            for (x, out) in out_row.iter_mut().enumerate() {
                let mut acc = 0.0f32;
                let mut idx = base + x;
                for &w in weights {
                    acc += horiz[idx] * w;
                    idx += stride;
                }
                *out = acc.round().clamp(0.0, 255.0) as u8;
            }
        });

    RgbImage::from_raw(dst_w, dst_h, out).expect("area_downscale buffer matches dimensions")
}

fn filter_type(filter: ResizeFilter) -> FilterType {
    match filter {
        ResizeFilter::Nearest => FilterType::Nearest,
        ResizeFilter::Triangle => FilterType::Triangle,
        ResizeFilter::CatmullRom => FilterType::CatmullRom,
        ResizeFilter::Lanczos3 => FilterType::Lanczos3,
    }
}

fn detector_tensor(input: &DetectorInput<'_>) -> Vec<f32> {
    let width = input.padded_size.width as usize;
    let height = input.padded_size.height as usize;
    let content_w = input.image.width() as usize;
    let content_h = input.image.height() as usize;
    let plane = width * height;
    let mut tensor = vec![0.0f32; DET_CHANNELS * plane];
    // Padding stays exactly 0.0 in normalized tensor space. That avoids creating
    // an artificial black border and avoids copying native 4K content into a
    // larger temporary image solely for shape alignment.
    let (c0, rest) = tensor.split_at_mut(plane);
    let (c1, c2) = rest.split_at_mut(plane);
    for (channel, plane_buf) in [(0usize, c0), (1, c1), (2, c2)] {
        plane_buf
            .par_chunks_mut(width)
            .enumerate()
            .take(content_h)
            .for_each(|(y, row)| {
                for (x, out) in row.iter_mut().take(content_w).enumerate() {
                    let value = input.image.get_pixel(x as u32, y as u32)[channel];
                    let normalized = value as f32 / 255.0;
                    *out = (normalized - DET_MEAN[channel]) / DET_STD[channel];
                }
            });
    }
    tensor
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct ScoredRect {
    rect: Rect,
    score: f32,
    pass: DetectorPass,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DetectorPass {
    Primary,
    LowContrast,
}

#[derive(Debug, Clone, Copy)]
struct DetectorPassConfig {
    probability_threshold: f32,
    box_score_threshold: f32,
    min_component_area: usize,
    close_radius_px: u32,
    unclip_ratio: f32,
    pass: DetectorPass,
    label: &'static str,
}

impl DetectorPassConfig {
    fn primary(config: &PipelineConfig) -> Self {
        Self {
            probability_threshold: config.detector_probability_threshold,
            box_score_threshold: config.detector_box_score_threshold,
            min_component_area: config.detector_min_component_area,
            close_radius_px: config.detector_close_radius_px,
            unclip_ratio: config.detector_unclip_ratio,
            pass: DetectorPass::Primary,
            label: "det.primary",
        }
    }

    fn low_contrast(config: &PipelineConfig) -> Self {
        Self {
            probability_threshold: config.detector_low_contrast_probability_threshold,
            box_score_threshold: config.detector_low_contrast_box_score_threshold,
            min_component_area: config.detector_min_component_area,
            close_radius_px: config.detector_close_radius_px,
            unclip_ratio: config.detector_unclip_ratio,
            pass: DetectorPass::LowContrast,
            label: "det.low_contrast",
        }
    }
}

impl DetectorPass {
    fn priority(self) -> u8 {
        match self {
            DetectorPass::Primary => 0,
            DetectorPass::LowContrast => 1,
        }
    }
}

fn detect_candidate_rects(
    detector: &mut Session,
    input_image: &DetectorInput,
    frame_size: Size,
    config: &PipelineConfig,
    pass_config: DetectorPassConfig,
) -> Result<Vec<ScoredRect>> {
    let label = pass_config.label;
    let tensor_data = prof(&format!("{label}.tensor"), || detector_tensor(input_image));
    detect_candidate_rects_with_tensor(
        detector,
        tensor_data,
        input_image,
        frame_size,
        config,
        pass_config,
    )
}

/// Run the detector on a prebuilt NCHW tensor and postprocess the probability
/// map in place. Split from [`detect_candidate_rects`] so the dual-pass path
/// can build the second pass's tensor on a worker thread while the first
/// pass's inference holds the session.
fn detect_candidate_rects_with_tensor(
    detector: &mut Session,
    tensor_data: Vec<f32>,
    input_image: &DetectorInput,
    frame_size: Size,
    config: &PipelineConfig,
    pass_config: DetectorPassConfig,
) -> Result<Vec<ScoredRect>> {
    let label = pass_config.label;
    let input = Tensor::from_array((
        [
            1usize,
            DET_CHANNELS,
            input_image.padded_size.height as usize,
            input_image.padded_size.width as usize,
        ],
        tensor_data.into_boxed_slice(),
    ))?;
    let outputs = prof(&format!("{label}.infer"), || {
        detector.run(ort::inputs![input])
    })?;
    let (map, map_w, map_h) = extract_probability_map(&outputs)?;
    Ok(prof(&format!("{label}.components"), || {
        candidates_from_probability_map(
            ProbabilityMap {
                data: map,
                width: map_w,
                height: map_h,
            },
            DetectorPostprocess {
                input_size: input_image.padded_size,
                content_size: input_image.content_size(),
                frame_size,
                config,
                pass_config,
            },
        )
    }))
}

/// Run the detector on a prebuilt tensor and return the probability map as the
/// owned output value, so postprocessing can move to another thread while the
/// session runs the next inference. `DynValue` owns its (host) buffer
/// independently of the session, so this hands the ~33 MB map across threads
/// without copying it.
fn detector_probability_value(
    detector: &mut Session,
    tensor_data: Vec<f32>,
    padded_size: Size,
    label: &str,
) -> Result<(ort::value::DynValue, usize, usize)> {
    let input = Tensor::from_array((
        [
            1usize,
            DET_CHANNELS,
            padded_size.height as usize,
            padded_size.width as usize,
        ],
        tensor_data.into_boxed_slice(),
    ))?;
    let outputs = prof(&format!("{label}.infer"), || {
        detector.run(ort::inputs![input])
    })?;
    let (_, map_w, map_h) = extract_probability_map(&outputs)?;
    let value = outputs
        .into_iter()
        .next()
        .map(|(_, value)| value)
        .context("detector produced no outputs")?;
    Ok((value, map_w, map_h))
}

fn extract_probability_map<'a>(
    outputs: &'a ort::session::SessionOutputs<'_>,
) -> Result<(&'a [f32], usize, usize)> {
    let (shape, data) = outputs[0].try_extract_tensor::<f32>()?;
    let dims = shape
        .iter()
        .map(|dim| usize::try_from(*dim).unwrap_or_default())
        .collect::<Vec<_>>();
    if dims.len() != 4 || dims[0] != 1 || dims[1] != 1 {
        bail!("unexpected detector output shape: {dims:?}");
    }
    Ok((data, dims[3], dims[2]))
}

struct ProbabilityMap<'a> {
    data: &'a [f32],
    width: usize,
    height: usize,
}

struct DetectorPostprocess<'a> {
    input_size: Size,
    content_size: Size,
    frame_size: Size,
    config: &'a PipelineConfig,
    pass_config: DetectorPassConfig,
}

fn candidates_from_probability_map(
    map: ProbabilityMap<'_>,
    context: DetectorPostprocess<'_>,
) -> Vec<ScoredRect> {
    let pass_config = context.pass_config;
    let mut candidates = detect_components_from_probability_map(
        map.data,
        map.width,
        map.height,
        pass_config.probability_threshold,
        pass_config.min_component_area,
        pass_config.close_radius_px,
        pass_config.unclip_ratio,
    );
    candidates.sort_by(|a, b| {
        b.score()
            .total_cmp(&a.score())
            .then_with(|| a.rect.y.total_cmp(&b.rect.y))
            .then_with(|| a.rect.x.total_cmp(&b.rect.x))
            .then_with(|| a.rect.width.total_cmp(&b.rect.width))
            .then_with(|| a.rect.height.total_cmp(&b.rect.height))
    });

    let map_size = Size::new(
        u32::try_from(map.width).unwrap_or(u32::MAX),
        u32::try_from(map.height).unwrap_or(u32::MAX),
    );
    candidates
        .into_iter()
        .filter(|candidate| candidate.score() >= pass_config.box_score_threshold)
        .filter_map(|candidate| {
            let rect = scale_detector_map_rect_to_frame(
                candidate.rect,
                context.input_size,
                context.content_size,
                map_size,
                context.frame_size,
            )?;
            Some(ScoredRect {
                rect,
                score: candidate.score(),
                pass: pass_config.pass,
            })
        })
        .filter(|candidate| {
            is_plausible_text_rect(candidate.rect, context.frame_size, context.config)
        })
        .collect()
}

fn scale_detector_map_rect_to_frame(
    rect: Rect,
    input_size: Size,
    content_size: Size,
    map_size: Size,
    frame_size: Size,
) -> Option<Rect> {
    let content_map_w = map_content_extent(map_size.width, content_size.width, input_size.width);
    let content_map_h = map_content_extent(map_size.height, content_size.height, input_size.height);
    let rect = clamp_rect_to_extent(rect, content_map_w, content_map_h)?;
    let sx = frame_size.width as f32 / content_map_w.max(1.0);
    let sy = frame_size.height as f32 / content_map_h.max(1.0);
    Some(rect.scale(sx, sy).clamp_to(frame_size))
}

fn map_content_extent(map_dim: u32, content_dim: u32, input_dim: u32) -> f32 {
    if map_dim == 0 || input_dim == 0 {
        return 1.0;
    }

    (map_dim as f32 * content_dim as f32 / input_dim as f32).max(1.0)
}

fn clamp_rect_to_extent(rect: Rect, max_w: f32, max_h: f32) -> Option<Rect> {
    let x = rect.x.clamp(0.0, max_w);
    let y = rect.y.clamp(0.0, max_h);
    let right = rect.right().clamp(0.0, max_w);
    let bottom = rect.bottom().clamp(0.0, max_h);
    let width = right - x;
    let height = bottom - y;

    (width > 0.0 && height > 0.0).then(|| Rect::new(x, y, width, height))
}

fn finalize_detection_boxes(
    candidates: Vec<ScoredRect>,
    frame_size: Size,
    config: &PipelineConfig,
    stable_ids: &mut StableIdAllocator,
    content_epoch: u64,
) -> Vec<TextBox> {
    let mut candidates = merge_detection_rects(candidates, frame_size, config);
    // When the cap binds, drop the least-confident boxes rather than whatever
    // happens to sit lowest on the screen.
    if candidates.len() > config.max_boxes_per_frame {
        candidates.sort_by(|a, b| b.score.total_cmp(&a.score));
        candidates.truncate(config.max_boxes_per_frame);
    }
    candidates.sort_by(|a, b| {
        a.rect
            .y
            .total_cmp(&b.rect.y)
            .then_with(|| a.rect.x.total_cmp(&b.rect.x))
            .then_with(|| a.rect.width.total_cmp(&b.rect.width))
            .then_with(|| a.rect.height.total_cmp(&b.rect.height))
    });

    candidates
        .into_iter()
        .map(|candidate| {
            let id = stable_ids.id_for(candidate.rect, STABLE_ID_BUCKET_PX);
            TextBox {
                id,
                rect: candidate.rect,
                confidence: candidate.score,
                content_fingerprint: content_epoch ^ id,
            }
        })
        .collect()
}

fn merge_detection_rects(
    mut candidates: Vec<ScoredRect>,
    frame_size: Size,
    config: &PipelineConfig,
) -> Vec<ScoredRect> {
    candidates.sort_by(|a, b| {
        a.pass
            .priority()
            .cmp(&b.pass.priority())
            .then_with(|| b.score.total_cmp(&a.score))
    });
    let mut merged: Vec<ScoredRect> = Vec::with_capacity(candidates.len());

    'candidate: for candidate in candidates {
        for existing in &mut merged {
            if should_dedupe_detection_rects(existing.rect, candidate.rect) {
                if candidate.pass == DetectorPass::Primary
                    && existing.pass != DetectorPass::Primary
                    && is_plausible_text_rect(candidate.rect, frame_size, config)
                {
                    existing.rect = candidate.rect;
                    existing.pass = candidate.pass;
                } else if candidate.pass == existing.pass && candidate.score > existing.score {
                    existing.rect = candidate.rect;
                }
                existing.score = existing.score.max(candidate.score);
                continue 'candidate;
            }
        }
        merged.push(candidate);
    }

    merged
}

fn should_dedupe_detection_rects(a: Rect, b: Rect) -> bool {
    if a.iou(b) >= DETECTION_DEDUPE_IOU {
        return true;
    }
    let intersection = intersection_area(a, b);
    if intersection <= 0.0 {
        return false;
    }
    let min_area = a.area().min(b.area()).max(1.0);
    intersection / min_area >= DETECTION_DEDUPE_CONTAINMENT
}

fn intersection_area(a: Rect, b: Rect) -> f32 {
    let left = a.x.max(b.x);
    let top = a.y.max(b.y);
    let right = a.right().min(b.right());
    let bottom = a.bottom().min(b.bottom());
    Rect::new(left, top, right - left, bottom - top).area()
}

/// Recognize one batch of similar-width line crops in a single inference, then
/// CTC-decode each row back into `results` at its original box index. Each row is
/// zero-padded (the recognizer's training pad value) from `real_w` to the batch's
/// `target_w`.
/// Pad-and-pack a width-sorted batch of crops into one NCHW tensor.
fn pack_recognizer_batch(crops: &[RecCrop], batch: &[usize]) -> (Vec<f32>, usize) {
    let rows = batch.len();
    let min_width = REC_MIN_WIDTH as usize;
    let target_w = batch
        .iter()
        .map(|&bi| crops[bi].real_w)
        .max()
        .unwrap_or(min_width)
        .next_multiple_of(REC_WIDTH_MULTIPLE as usize)
        .max(min_width);

    let height = REC_INPUT_HEIGHT as usize;
    let mut tensor = vec![0.0f32; rows * REC_CHANNELS * height * target_w];
    for (row, &bi) in batch.iter().enumerate() {
        write_recognizer_row(&mut tensor, row, &crops[bi].resized, target_w);
    }
    (tensor, target_w)
}

/// CTC-decode one inferred batch. Rows decode in parallel: the per-timestep
/// argmax over the large recognizer charset is a meaningful CPU cost, and rows
/// are independent.
fn decode_recognizer_value(
    value: &ort::value::DynValue,
    dictionary: &[String],
    crops: &[RecCrop],
    batch: &[usize],
    boxes: &[TextBox],
    target_w: usize,
) -> Result<Vec<(usize, RecognizedText)>> {
    let (shape, data) = value.try_extract_tensor::<f32>()?;
    let dims = shape
        .iter()
        .map(|dim| usize::try_from(*dim).unwrap_or_default())
        .collect::<Vec<_>>();
    if dims.len() != 3 || dims[0] != batch.len() {
        bail!(
            "unexpected recognizer output shape: {dims:?} (expected batch {})",
            batch.len()
        );
    }
    let timesteps = dims[1];
    let classes = dims[2];

    Ok(batch
        .par_iter()
        .enumerate()
        .map(|(row, &bi)| {
            let crop = &crops[bi];
            let logits = &data[row * timesteps * classes..(row + 1) * timesteps * classes];
            let (raw_text, confidence, fractions) =
                decode_ctc(logits, timesteps, classes, dictionary);
            // Timesteps span the padded width `target_w`; rescale each glyph
            // fraction back onto the real (unpadded) crop, clamped so any stray
            // pad-region emission can't land past the line.
            let scale = target_w as f32 / crop.real_w.max(1) as f32;
            let raw_centers: Vec<f32> = fractions
                .iter()
                .map(|fraction| {
                    crop.crop_x as f32 + (fraction * scale).min(1.0) * crop.crop_w as f32
                })
                .collect();
            let text = normalize_recognized_text(&raw_text);
            let char_centers = align_centers(&raw_text, &raw_centers, &text);
            let text_box = adjust_text_box_for_normalization(
                &boxes[crop.index],
                &raw_text,
                &text,
                &raw_centers,
            );
            (
                crop.index,
                RecognizedText {
                    text_box,
                    text,
                    confidence,
                    reused: false,
                    char_centers,
                },
            )
        })
        .collect())
}

fn run_recognizer_infer(
    recognizer: &mut Session,
    tensor: Vec<f32>,
    rows: usize,
    target_w: usize,
) -> Result<ort::value::DynValue> {
    let input = Tensor::from_array((
        [rows, REC_CHANNELS, REC_INPUT_HEIGHT as usize, target_w],
        tensor.into_boxed_slice(),
    ))?;
    let outputs = prof(&format!("  rec.infer[n={rows} w={target_w}]"), || {
        recognizer.run(ort::inputs![input])
    })?;
    outputs
        .into_iter()
        .next()
        .map(|(_, value)| value)
        .context("recognizer produced no outputs")
}

fn run_recognizer_batch(
    recognizer: &mut Session,
    dictionary: &[String],
    crops: &[RecCrop],
    batch: &[usize],
    boxes: &[TextBox],
    results: &mut [Option<RecognizedText>],
) -> Result<()> {
    if batch.is_empty() {
        return Ok(());
    }
    let (tensor, target_w) = pack_recognizer_batch(crops, batch);
    let value = run_recognizer_infer(recognizer, tensor, batch.len(), target_w)?;
    for (index, recognized) in
        decode_recognizer_value(&value, dictionary, crops, batch, boxes, target_w)?
    {
        results[index] = Some(recognized);
    }
    Ok(())
}

/// Run every batch through a three-stage pipeline: a packer thread builds the
/// next batch's tensor and a decoder thread CTC-decodes the previous batch's
/// logits while the session thread keeps the GPU busy with inference. Batch
/// composition and all per-batch math are identical to the sequential path;
/// only the scheduling differs. The output `DynValue` owns its buffer, so
/// logits cross to the decoder thread without copying.
fn run_recognizer_batches_pipelined(
    recognizer: &mut Session,
    dictionary: &[String],
    crops: &[RecCrop],
    order: &[usize],
    boxes: &[TextBox],
    results: &mut [Option<RecognizedText>],
) -> Result<()> {
    if order.is_empty() {
        return Ok(());
    }

    struct PackedBatch {
        batch: Vec<usize>,
        tensor: Vec<f32>,
        target_w: usize,
    }
    struct InferredBatch {
        batch: Vec<usize>,
        value: ort::value::DynValue,
        target_w: usize,
    }

    let decoded = std::thread::scope(|scope| -> Result<Vec<(usize, RecognizedText)>> {
        let (pack_tx, pack_rx) = std::sync::mpsc::sync_channel::<PackedBatch>(1);
        let (decode_tx, decode_rx) = std::sync::mpsc::sync_channel::<InferredBatch>(2);

        scope.spawn(move || {
            for batch in order.chunks(REC_BATCH_MAX) {
                let (tensor, target_w) = pack_recognizer_batch(crops, batch);
                let packed = PackedBatch {
                    batch: batch.to_vec(),
                    tensor,
                    target_w,
                };
                // The session thread hanging up early (on error) ends packing.
                if pack_tx.send(packed).is_err() {
                    return;
                }
            }
        });

        let decoder = scope.spawn(move || -> Result<Vec<(usize, RecognizedText)>> {
            let mut decoded = Vec::new();
            while let Ok(inferred) = decode_rx.recv() {
                decoded.extend(decode_recognizer_value(
                    &inferred.value,
                    dictionary,
                    crops,
                    &inferred.batch,
                    boxes,
                    inferred.target_w,
                )?);
            }
            Ok(decoded)
        });

        let mut session_result: Result<()> = Ok(());
        while let Ok(packed) = pack_rx.recv() {
            let rows = packed.batch.len();
            match run_recognizer_infer(recognizer, packed.tensor, rows, packed.target_w) {
                Ok(value) => {
                    let inferred = InferredBatch {
                        batch: packed.batch,
                        value,
                        target_w: packed.target_w,
                    };
                    if decode_tx.send(inferred).is_err() {
                        break;
                    }
                }
                Err(error) => {
                    session_result = Err(error);
                    break;
                }
            }
        }
        drop(pack_rx);
        drop(decode_tx);
        let decoded = decoder
            .join()
            .map_err(|_| anyhow::anyhow!("recognizer decode thread panicked"))?;
        session_result?;
        decoded
    })?;

    for (index, recognized) in decoded {
        results[index] = Some(recognized);
    }
    Ok(())
}

fn adjust_text_box_for_normalization(
    original: &TextBox,
    raw_text: &str,
    normalized_text: &str,
    raw_centers: &[f32],
) -> TextBox {
    if raw_text.is_empty() {
        return original.clone();
    }
    if let Some(removed) = removed_leading_timer_badge_len(raw_text, normalized_text) {
        // Only move the edge for glyphs that actually sat inside the detector
        // box; decoded junk from the crop padding outside it must not shrink
        // the box.
        if removed_prefix_inside_box(original, raw_centers, removed) {
            return shrink_text_box_left_to_kept_prefix(original, raw_centers, removed);
        }
        return original.clone();
    }
    if let Some((start, end)) = kept_span_after_removed_trailing_ui_value(raw_text, normalized_text)
    {
        if removed_edges_inside_box(original, raw_centers, start, end) {
            return shrink_text_box_to_kept_span(original, raw_centers, start, end);
        }
        return original.clone();
    }
    if has_inserted_trailing_punctuation(raw_text, normalized_text) {
        return expand_text_box_right_for_inserted_suffix(original, raw_text);
    }
    if !has_inserted_leading_opener(raw_text, normalized_text) {
        return original.clone();
    }

    let raw_len = raw_text.chars().count().max(1) as f32;
    let pad = (original.rect.width / raw_len).clamp(4.0, original.rect.height.max(4.0));
    let left = (original.rect.x - pad).max(0.0);
    let right = original.rect.right();
    let mut adjusted = original.clone();
    adjusted.rect.x = left;
    adjusted.rect.width = right - left;
    adjusted
}

/// Whether any removed leading glyph's center lies inside the detector box
/// (with a small slack); pad-region junk outside it must not move edges.
fn removed_prefix_inside_box(original: &TextBox, raw_centers: &[f32], removed: usize) -> bool {
    let slack = original.rect.height * 0.1;
    raw_centers
        .iter()
        .take(removed)
        .any(|&center| center >= original.rect.x + slack)
}

/// Whether the glyphs removed at either edge of the kept span sat inside the
/// detector box.
fn removed_edges_inside_box(
    original: &TextBox,
    raw_centers: &[f32],
    start: usize,
    end: usize,
) -> bool {
    let slack = original.rect.height * 0.1;
    let leading_inside = raw_centers
        .iter()
        .take(start)
        .any(|&center| center >= original.rect.x + slack);
    let trailing_inside = raw_centers
        .iter()
        .skip(end)
        .any(|&center| center <= original.rect.right() - slack);
    leading_inside || trailing_inside || raw_centers.is_empty()
}

fn removed_leading_timer_badge_len(raw_text: &str, normalized_text: &str) -> Option<usize> {
    let raw_chars = raw_text.chars().collect::<Vec<_>>();
    let first = raw_chars.first().copied()?;
    if !first.is_ascii_uppercase() || !looks_like_japanese_duration(normalized_text) {
        return None;
    }

    let mut index = 1usize;
    while raw_chars.get(index).is_some_and(|ch| ch.is_whitespace()) {
        index += 1;
    }
    let suffix = raw_chars[index..].iter().collect::<String>();
    (suffix == normalized_text).then_some(index)
}

fn kept_span_after_removed_trailing_ui_value(
    raw_text: &str,
    normalized_text: &str,
) -> Option<(usize, usize)> {
    let raw_chars = raw_text.chars().collect::<Vec<_>>();
    let normalized_chars = normalized_text.chars().collect::<Vec<_>>();
    let kept = normalized_chars.len();
    if kept == 0 || kept >= raw_chars.len() {
        return None;
    }
    for start in 0..=1 {
        let end = start + kept;
        if end > raw_chars.len() || raw_chars[start..end] != normalized_chars {
            continue;
        }
        let suffix = raw_chars[end..].iter().collect::<String>();
        if is_removed_trailing_ui_value(&suffix) {
            return Some((start, end));
        }
    }
    None
}

fn is_removed_trailing_ui_value(suffix: &str) -> bool {
    if let Some(level) = suffix.strip_prefix('+') {
        return !level.is_empty() && level.chars().all(|ch| ch.is_ascii_digit());
    }
    if is_removed_trailing_ratio(suffix) {
        return true;
    }
    let suffix = suffix.trim_start_matches('・').trim();
    if suffix.is_empty() {
        return false;
    }
    let number = suffix.strip_suffix('%').unwrap_or(suffix);
    let mut saw_digit = false;
    let mut saw_dot = false;
    for ch in number.chars() {
        if ch.is_ascii_digit() {
            saw_digit = true;
        } else if ch == '.' && !saw_dot {
            saw_dot = true;
        } else {
            return false;
        }
    }
    saw_digit
}

fn is_removed_trailing_ratio(suffix: &str) -> bool {
    let Some((left, right)) = suffix.split_once('/') else {
        return false;
    };
    !left.is_empty()
        && !right.is_empty()
        && left.chars().all(|ch| ch.is_ascii_digit())
        && right.chars().all(|ch| ch.is_ascii_digit())
}

fn looks_like_japanese_duration(text: &str) -> bool {
    let chars = text.chars().collect::<Vec<_>>();
    let mut index = 0usize;
    let day_digits = consume_ascii_digits(&chars, &mut index);
    day_digits > 0
        && chars.get(index) == Some(&'日')
        && {
            index += 1;
            consume_ascii_digits(&chars, &mut index) > 0
        }
        && chars.get(index) == Some(&'時')
        && chars.get(index + 1) == Some(&'間')
        && index + 2 == chars.len()
}

fn consume_ascii_digits(chars: &[char], index: &mut usize) -> usize {
    let start = *index;
    while chars.get(*index).is_some_and(|ch| ch.is_ascii_digit()) {
        *index += 1;
    }
    *index - start
}

fn shrink_text_box_left_to_kept_prefix(
    original: &TextBox,
    raw_centers: &[f32],
    removed: usize,
) -> TextBox {
    if removed == 0 {
        return original.clone();
    }
    let right = original.rect.right();
    let left = if removed < raw_centers.len() {
        let kept_center = raw_centers[removed];
        let half_kept_width = raw_centers
            .get(removed + 1)
            .map(|next| (next - kept_center).abs() / 2.0)
            .unwrap_or_else(|| (kept_center - raw_centers[removed - 1]).abs() / 2.0);
        (kept_center - half_kept_width).clamp(original.rect.x, right)
    } else {
        let raw_len = raw_centers.len().max(removed).max(1) as f32;
        (original.rect.x + original.rect.width * removed as f32 / raw_len)
            .clamp(original.rect.x, right)
    };
    let mut adjusted = original.clone();
    adjusted.rect.x = left;
    adjusted.rect.width = (right - left).max(1.0);
    adjusted
}

fn shrink_text_box_to_kept_span(
    original: &TextBox,
    raw_centers: &[f32],
    start: usize,
    end: usize,
) -> TextBox {
    if start >= end {
        return original.clone();
    }
    let raw_len = raw_centers.len().max(end).max(1);
    let original_right = original.rect.right();
    let left = if start == 0 {
        original.rect.x
    } else if start < raw_centers.len() {
        let kept_center = raw_centers[start];
        let half_kept_width = raw_centers
            .get(start + 1)
            .map(|next| (next - kept_center).abs() / 2.0)
            .or_else(|| {
                start
                    .checked_sub(1)
                    .map(|previous| (kept_center - raw_centers[previous]).abs() / 2.0)
            })
            .unwrap_or_else(|| original.rect.width / raw_len as f32 / 2.0);
        (kept_center - half_kept_width).clamp(original.rect.x, original_right)
    } else {
        (original.rect.x + original.rect.width * start as f32 / raw_len as f32)
            .clamp(original.rect.x, original_right)
    };
    let right = if end < raw_centers.len() {
        let kept_center = raw_centers[end - 1];
        let half_kept_width = raw_centers
            .get(end)
            .map(|next| (next - kept_center).abs() / 2.0)
            .or_else(|| {
                end.checked_sub(2)
                    .map(|previous| (kept_center - raw_centers[previous]).abs() / 2.0)
            })
            .unwrap_or_else(|| original.rect.width / raw_len as f32 / 2.0);
        (kept_center + half_kept_width).clamp(left + 1.0, original_right)
    } else {
        original_right
    };
    let mut adjusted = original.clone();
    adjusted.rect.x = left;
    adjusted.rect.width = (right - left).max(1.0);
    adjusted
}

fn has_inserted_leading_opener(raw_text: &str, normalized_text: &str) -> bool {
    let Some(first) = normalized_text.chars().next() else {
        return false;
    };
    matches!(first, '「' | '『' | '【') && !raw_text.trim_start().starts_with(first)
}

fn has_inserted_trailing_punctuation(raw_text: &str, normalized_text: &str) -> bool {
    if raw_text.chars().count() > 20 {
        return false;
    }
    let Some(suffix) = normalized_text.strip_prefix(raw_text) else {
        return false;
    };
    !suffix.is_empty() && suffix.chars().all(|ch| matches!(ch, '。' | '、'))
}

fn expand_text_box_right_for_inserted_suffix(original: &TextBox, raw_text: &str) -> TextBox {
    let raw_len = raw_text.chars().count().max(1) as f32;
    let pad = (original.rect.width / raw_len).clamp(4.0, original.rect.height.max(4.0));
    let mut adjusted = original.clone();
    adjusted.rect.width += pad;
    adjusted
}

/// Write one resized line crop into row `row` of an NCHW recognizer batch tensor,
/// normalized to `[-1, 1]`. Columns past the crop's width stay at the zero pad the
/// caller initialized.
fn write_recognizer_row(tensor: &mut [f32], row: usize, image: &RgbImage, target_w: usize) {
    let height = REC_INPUT_HEIGHT as usize;
    let copy_w = (image.width() as usize).min(target_w);
    for y in 0..height.min(image.height() as usize) {
        for x in 0..copy_w {
            let pixel = image.get_pixel(x as u32, y as u32);
            for channel in 0..REC_CHANNELS {
                let value = (pixel[channel] as f32 / 255.0 - 0.5) / 0.5;
                let index = ((row * REC_CHANNELS + channel) * height + y) * target_w + x;
                tensor[index] = value;
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
struct DetectionCandidate {
    rect: Rect,
    area: usize,
    score_sum: f32,
}

impl DetectionCandidate {
    fn score(&self) -> f32 {
        if self.area == 0 {
            0.0
        } else {
            self.score_sum / self.area as f32
        }
    }
}

/// One horizontal run of mask-true pixels, with its union-find run id.
#[derive(Debug, Clone, Copy)]
struct MaskRun {
    row: u32,
    start: u32,
    end: u32,
    score_sum: f32,
}

fn detect_components_from_probability_map(
    data: &[f32],
    width: usize,
    height: usize,
    threshold: f32,
    min_component_area: usize,
    close_radius_px: u32,
    unclip_ratio: f32,
) -> Vec<DetectionCandidate> {
    // Scanline connected components with union-find over row runs. Produces
    // exactly the same components (bounding box, area, score sum) as a
    // per-pixel flood fill under 4-connectivity, but touches each pixel once
    // in row order, which is several times faster on 4K probability maps.
    let mask = detector_component_mask(data, width, height, threshold, close_radius_px);
    let min_area = min_component_area.max(1);

    let mut runs: Vec<MaskRun> = Vec::new();
    let mut parent: Vec<u32> = Vec::new();
    // Run-index ranges of the previous and current row.
    let mut previous_row: std::ops::Range<usize> = 0..0;

    fn find(parent: &mut [u32], mut node: u32) -> u32 {
        while parent[node as usize] != node {
            let grandparent = parent[parent[node as usize] as usize];
            parent[node as usize] = grandparent;
            node = grandparent;
        }
        node
    }

    for y in 0..height {
        let row_offset = y * width;
        let row_start = runs.len();
        let mut x = 0usize;
        while x < width {
            if !mask[row_offset + x] {
                x += 1;
                continue;
            }
            let start = x;
            let mut score_sum = 0.0f32;
            while x < width && mask[row_offset + x] {
                score_sum += data[row_offset + x].clamp(0.0, 1.0);
                x += 1;
            }
            let id = runs.len() as u32;
            runs.push(MaskRun {
                row: y as u32,
                start: start as u32,
                end: x as u32,
                score_sum,
            });
            parent.push(id);
        }
        let current_row_start = row_start;
        let current_row_end = runs.len();

        // Union with 4-connected overlapping runs of the previous row. Both
        // run lists are sorted by start column; sweep them together.
        let mut above = previous_row.start;
        for current in current_row_start..current_row_end {
            let (cur_start, cur_end) = (runs[current].start, runs[current].end);
            while above < previous_row.end && runs[above].end <= cur_start {
                above += 1;
            }
            let mut probe = above;
            while probe < previous_row.end && runs[probe].start < cur_end {
                let a = find(&mut parent, current as u32);
                let b = find(&mut parent, probe as u32);
                if a != b {
                    parent[a.max(b) as usize] = a.min(b);
                }
                if runs[probe].end >= cur_end {
                    break;
                }
                probe += 1;
            }
        }
        previous_row = current_row_start..current_row_end;
    }

    // Fold runs into per-root accumulators.
    #[derive(Clone, Copy)]
    struct Accumulator {
        min_x: u32,
        max_x: u32,
        min_y: u32,
        max_y: u32,
        area: usize,
        score_sum: f32,
    }
    let mut accumulators: HashMap<u32, Accumulator> = HashMap::new();
    for (index, &run) in runs.iter().enumerate() {
        let root = find(&mut parent, index as u32);
        let entry = accumulators.entry(root).or_insert(Accumulator {
            min_x: run.start,
            max_x: run.end - 1,
            min_y: run.row,
            max_y: run.row,
            area: 0,
            score_sum: 0.0,
        });
        entry.min_x = entry.min_x.min(run.start);
        entry.max_x = entry.max_x.max(run.end - 1);
        entry.min_y = entry.min_y.min(run.row);
        entry.max_y = entry.max_y.max(run.row);
        entry.area += (run.end - run.start) as usize;
        entry.score_sum += run.score_sum;
    }

    let mut roots: Vec<u32> = accumulators.keys().copied().collect();
    roots.sort_unstable();
    roots
        .into_iter()
        .filter_map(|root| {
            let acc = accumulators[&root];
            if acc.area < min_area {
                return None;
            }
            let rect = unclip_detector_rect(
                Rect::new(
                    acc.min_x as f32,
                    acc.min_y as f32,
                    (acc.max_x - acc.min_x + 1) as f32,
                    (acc.max_y - acc.min_y + 1) as f32,
                ),
                acc.area,
                width,
                height,
                unclip_ratio,
            );
            Some(DetectionCandidate {
                rect,
                area: acc.area,
                score_sum: acc.score_sum,
            })
        })
        .collect()
}

fn detector_component_mask(
    data: &[f32],
    width: usize,
    height: usize,
    threshold: f32,
    close_radius_px: u32,
) -> Vec<bool> {
    let mask = data
        .iter()
        .map(|value| *value >= threshold)
        .collect::<Vec<_>>();
    horizontal_close_mask(mask, width, height, close_radius_px as usize)
}

fn horizontal_close_mask(mask: Vec<bool>, width: usize, height: usize, radius: usize) -> Vec<bool> {
    if radius == 0 || width == 0 || height == 0 {
        return mask;
    }

    let mut dilated = vec![false; mask.len()];
    for y in 0..height {
        let row = y * width;
        let mut prefix = vec![0usize; width + 1];
        for x in 0..width {
            prefix[x + 1] = prefix[x] + usize::from(mask[row + x]);
        }
        for x in 0..width {
            let window_start = x.saturating_sub(radius);
            let window_end = (x + radius).min(width - 1);
            let active = prefix[window_end + 1] - prefix[window_start];
            dilated[row + x] = active > 0;
        }
    }

    let mut closed = vec![false; mask.len()];
    for y in 0..height {
        let row = y * width;
        let mut prefix = vec![0usize; width + 1];
        for x in 0..width {
            prefix[x + 1] = prefix[x] + usize::from(dilated[row + x]);
        }
        for x in 0..width {
            let window_start = x.saturating_sub(radius);
            let window_end = (x + radius).min(width - 1);
            let window_len = window_end - window_start + 1;
            let active = prefix[window_end + 1] - prefix[window_start];
            closed[row + x] = active == window_len;
        }
    }

    closed
}

fn unclip_detector_rect(
    rect: Rect,
    component_area: usize,
    map_width: usize,
    map_height: usize,
    unclip_ratio: f32,
) -> Rect {
    if unclip_ratio <= 0.0 || component_area == 0 {
        return rect;
    }
    let perimeter = (2.0 * (rect.width + rect.height)).max(1.0);
    let distance = unclip_ratio * component_area as f32 / perimeter;
    Rect::new(
        rect.x - distance,
        rect.y - distance,
        rect.width + 2.0 * distance,
        rect.height + 2.0 * distance,
    )
    .clamp_to(Size::new(
        u32::try_from(map_width).unwrap_or(u32::MAX),
        u32::try_from(map_height).unwrap_or(u32::MAX),
    ))
}

fn is_plausible_text_rect(
    rect: Rect,
    frame_size: crate::geometry::Size,
    config: &PipelineConfig,
) -> bool {
    if rect.width < MIN_TEXT_RECT_SIDE_PX
        || rect.height < MIN_TEXT_RECT_SIDE_PX
        || rect.area() < MIN_TEXT_RECT_AREA_PX
    {
        return false;
    }

    let frame_area = (frame_size.width as f32 * frame_size.height as f32).max(1.0);
    let max_height = frame_size.height as f32 * config.detector_max_box_height_ratio.max(0.0);
    let max_area = frame_area * config.detector_max_box_area_ratio.max(0.0);
    rect.height <= max_height.max(1.0) && rect.area() <= max_area.max(1.0)
}

/// The padded crop region the recognizer sees for a text box, in original frame
/// pixels. Shared by the crop and by per-glyph position mapping so both agree on
/// the horizontal extent.
#[derive(Debug, Clone, Copy)]
struct CropBounds {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

/// Padding around the detected rect, as a fraction of its height (with a
/// floor). The detector's component boxes routinely clip a weak edge glyph
/// (trailing 。/：, leading brackets); at 0.5h the recognizer sees most of
/// that glyph's body and decodes it, while the scored box stays the
/// detector's. Measured on the eval corpus, the recovered edge glyphs
/// outweigh the occasional neighbor-glyph bleed this margin admits.
const CROP_PAD_RATIO: f32 = 0.50;
const CROP_PAD_MIN_PX: f32 = 3.0;

fn crop_bounds(image_w: u32, image_h: u32, rect: Rect) -> CropBounds {
    let padding = (rect.height * CROP_PAD_RATIO).max(CROP_PAD_MIN_PX);
    let x = (rect.x - padding).floor().max(0.0) as u32;
    let y = (rect.y - padding).floor().max(0.0) as u32;
    let right = (rect.x + rect.width + padding).ceil().min(image_w as f32) as u32;
    let bottom = (rect.y + rect.height + padding).ceil().min(image_h as f32) as u32;
    CropBounds {
        x,
        y,
        width: right.saturating_sub(x).max(1),
        height: bottom.saturating_sub(y).max(1),
    }
}

fn crop_text_line(image: &RgbImage, rect: Rect) -> RgbImage {
    let bounds = crop_bounds(image.width(), image.height(), rect);
    imageops::crop_imm(image, bounds.x, bounds.y, bounds.width, bounds.height).to_image()
}

/// Map the per-raw-character x-centres onto the normalized text by walking both
/// strings in order. The recognizer's CTC positions describe the raw decoded
/// string, but the displayed/segmented text is normalized; this recovers
/// centres parallel to `normalized.chars()`. Returns empty if the two can't be
/// aligned cleanly (heavily rewritten English UI lines), so hover then falls
/// back to even spacing. For Japanese text normalization is ~1:1, so alignment
/// succeeds and positions are exact.
fn align_centers(raw: &str, raw_centers: &[f32], normalized: &str) -> Vec<f32> {
    let raw_chars: Vec<char> = raw.chars().collect();
    if raw_chars.len() != raw_centers.len() || raw_chars.is_empty() {
        return Vec::new();
    }

    let mut out: Vec<f32> = Vec::with_capacity(normalized.chars().count());
    let mut r = 0usize;
    for nc in normalized.chars() {
        if nc == ' ' {
            // Spaces are inserted structurally (Latin boundaries, collapse) and
            // have no raw counterpart; interpolate their position afterwards.
            out.push(f32::NAN);
            continue;
        }
        let mut matched = None;
        while r < raw_chars.len() {
            let rc = raw_chars[r];
            r += 1;
            if char_matches(rc, nc) {
                matched = Some(raw_centers[r - 1]);
                break;
            }
        }
        match matched {
            Some(center) => out.push(center),
            None => return Vec::new(),
        }
    }

    fill_center_gaps(&mut out);
    out
}

/// Whether a raw decoded char corresponds to a normalized char, accounting for
/// the 1:1 substitutions `normalize_recognized_text` applies.
fn char_matches(raw: char, normalized: char) -> bool {
    raw == normalized || normalize_single_char(raw) == Some(normalized)
}

fn normalize_single_char(ch: char) -> Option<char> {
    match ch {
        '\u{3000}' => Some(' '),
        '／' => Some('/'),
        '－' => Some('-'),
        '％' => Some('%'),
        '·' => Some('・'),
        '查' => Some('査'),
        '换' => Some('換'),
        '壳' => Some('売'),
        '埗' => Some('捗'),
        '擊' => Some('撃'),
        '髓' => Some('髄'),
        '每' => Some('毎'),
        '增' => Some('増'),
        '剂' => Some('剤'),
        '对' => Some('対'),
        '载' => Some('載'),
        '费' => Some('費'),
        '济' => Some('済'),
        '电' => Some('電'),
        '鸣' => Some('鳴'),
        '测' => Some('測'),
        '值' => Some('値'),
        '銳' => Some('鋭'),
        '齐' => Some('斉'),
        _ => None,
    }
}

/// Replace NaN placeholders (inserted spaces) with the midpoint of their
/// nearest real neighbours so every entry is a usable coordinate.
fn fill_center_gaps(centers: &mut [f32]) {
    let len = centers.len();
    for i in 0..len {
        if !centers[i].is_nan() {
            continue;
        }
        let left = (0..i)
            .rev()
            .find(|&j| !centers[j].is_nan())
            .map(|j| centers[j]);
        let right = (i + 1..len)
            .find(|&j| !centers[j].is_nan())
            .map(|j| centers[j]);
        centers[i] = match (left, right) {
            (Some(l), Some(r)) => (l + r) / 2.0,
            (Some(l), None) => l,
            (None, Some(r)) => r,
            (None, None) => 0.0,
        };
    }
}

fn resize_for_recognizer(image: &RgbImage) -> RgbImage {
    let ratio = image.width() as f32 / image.height().max(1) as f32;
    let width = ((REC_INPUT_HEIGHT as f32 * ratio).round() as u32)
        .clamp(REC_MIN_WIDTH, REC_MAX_WIDTH)
        .next_multiple_of(REC_WIDTH_MULTIPLE);
    // Bilinear matches the recognizer's training-time preprocessing; bicubic
    // upscaling was measured slightly worse on the eval corpus.
    imageops::resize(image, width, REC_INPUT_HEIGHT, FilterType::Triangle)
}

/// Greedy CTC decode. Returns the decoded string, mean confidence, and a
/// per-character fraction in `[0, 1]` giving each glyph's horizontal position
/// along the recognizer input (and hence the crop) width, taken from the
/// timestep at which the glyph was emitted.
fn decode_ctc(
    data: &[f32],
    timesteps: usize,
    classes: usize,
    dictionary: &[String],
) -> (String, f32, Vec<f32>) {
    let mut out = String::new();
    let mut fractions = Vec::new();
    let mut last_index = 0usize;
    let mut confidence_sum = 0.0f32;
    let mut confidence_count = 0usize;

    for t in 0..timesteps {
        let offset = t * classes;
        let mut best_index = 0usize;
        let mut best_value = f32::NEG_INFINITY;
        for class in 0..classes {
            let value = data[offset + class];
            if value > best_value {
                best_value = value;
                best_index = class;
            }
        }

        if best_index != 0
            && best_index != last_index
            && let Some(ch) = dictionary.get(best_index - 1)
        {
            let fraction = (t as f32 + 0.5) / timesteps.max(1) as f32;
            for c in ch.chars() {
                out.push(c);
                fractions.push(fraction);
            }
            confidence_sum += best_value.clamp(0.0, 1.0);
            confidence_count += 1;
        }
        last_index = best_index;
    }

    let confidence = if confidence_count == 0 {
        0.0
    } else {
        confidence_sum / confidence_count as f32
    };
    (out, confidence, fractions)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_connected_rectangles() {
        let mut map = vec![0.0; 10 * 8];
        for y in 2..5 {
            for x in 3..7 {
                map[y * 10 + x] = 0.9;
            }
        }
        let config = PipelineConfig::default();
        let candidates = detect_components_from_probability_map(
            &map,
            10,
            8,
            config.detector_probability_threshold,
            config.detector_min_component_area,
            0,
            0.0,
        );
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].rect, Rect::new(3.0, 2.0, 4.0, 3.0));
        assert!((candidates[0].score() - 0.9).abs() < 0.001);
    }

    #[test]
    fn closes_horizontal_gaps_and_unclips_detector_components() {
        let mut map = vec![0.0; 12 * 6];
        for x in 2..5 {
            map[2 * 12 + x] = 0.9;
        }
        for x in 7..10 {
            map[2 * 12 + x] = 0.9;
        }

        let without_close = detect_components_from_probability_map(&map, 12, 6, 0.3, 1, 0, 0.0);
        assert_eq!(without_close.len(), 2);

        let with_close = detect_components_from_probability_map(&map, 12, 6, 0.3, 1, 1, 0.0);
        assert_eq!(with_close.len(), 1);
        assert_eq!(with_close[0].rect, Rect::new(2.0, 2.0, 8.0, 1.0));

        let unclipped = detect_components_from_probability_map(&map, 12, 6, 0.3, 1, 1, 1.5);
        assert_eq!(unclipped.len(), 1);
        assert!(unclipped[0].rect.x < with_close[0].rect.x);
        assert!(unclipped[0].rect.width > with_close[0].rect.width);
    }

    #[test]
    fn decodes_ctc_sequence() {
        let dict = vec!["A".to_string(), "B".to_string()];
        let data = [
            0.1, 0.8, 0.1, // A
            0.1, 0.9, 0.0, // repeated A
            0.8, 0.1, 0.1, // blank
            0.1, 0.2, 0.7, // B
        ];
        let (text, confidence, fractions) = decode_ctc(&data, 4, 3, &dict);
        assert_eq!(text, "AB");
        assert!(confidence > 0.7);
        // 'A' emitted at t=0, 'B' at t=3 (of 4 timesteps).
        assert_eq!(fractions.len(), 2);
        assert!((fractions[0] - 0.125).abs() < 1e-6);
        assert!((fractions[1] - 0.875).abs() < 1e-6);
    }

    #[test]
    fn align_centers_maps_identity_japanese() {
        // Pure Japanese: normalization is identity, so centres pass through.
        let raw = "水を飲む";
        let centers = [10.0, 20.0, 30.0, 40.0];
        let aligned = align_centers(raw, &centers, raw);
        assert_eq!(aligned, vec![10.0, 20.0, 30.0, 40.0]);
    }

    #[test]
    fn align_centers_interpolates_inserted_space() {
        // A space inserted by normalization (no raw counterpart) is filled with
        // the midpoint of its neighbours.
        let raw = "AB";
        let centers = [10.0, 30.0];
        let aligned = align_centers(raw, &centers, "A B");
        assert_eq!(aligned, vec![10.0, 20.0, 30.0]);
    }

    #[test]
    fn align_centers_handles_fullwidth_substitution() {
        // Full-width percent in raw maps to ASCII '%' in normalized (1:1).
        let raw = "A％B";
        let centers = [10.0, 20.0, 30.0];
        let aligned = align_centers(raw, &centers, "A%B");
        assert_eq!(aligned, vec![10.0, 20.0, 30.0]);
    }

    #[test]
    fn align_centers_bails_when_unalignable() {
        // Normalized contains a char with no raw counterpart in order: fall back.
        let raw = "AB";
        let centers = [10.0, 30.0];
        assert!(align_centers(raw, &centers, "AXB").is_empty());
    }

    #[test]
    fn filters_low_score_detector_components() {
        let mut config = PipelineConfig {
            detector_box_score_threshold: 0.5,
            ..PipelineConfig::default()
        };
        let mut low = DetectionCandidate {
            rect: Rect::new(0.0, 0.0, 4.0, 4.0),
            area: 16,
            score_sum: 7.2,
        };
        assert!(low.score() < config.detector_box_score_threshold);

        low.score_sum = 8.8;
        config.detector_box_score_threshold = 0.5;
        assert!(low.score() > config.detector_box_score_threshold);
    }

    #[test]
    fn rejects_oversized_non_text_rectangles() {
        let config = PipelineConfig::default();
        let frame_size = crate::geometry::Size::new(3840, 2160);

        assert!(is_plausible_text_rect(
            Rect::new(2800.0, 690.0, 640.0, 48.0),
            frame_size,
            &config
        ));
        assert!(!is_plausible_text_rect(
            Rect::new(1236.0, 330.0, 316.0, 369.0),
            frame_size,
            &config
        ));
    }

    #[test]
    fn expands_box_when_normalization_inserts_leading_opener() {
        let original = TextBox {
            id: 7,
            rect: Rect::new(100.0, 20.0, 120.0, 24.0),
            confidence: 0.9,
            content_fingerprint: 7,
        };

        let adjusted = adjust_text_box_for_normalization(
            &original,
            "砕けた記憶」",
            "「砕けた記憶」",
            &[105.0, 125.0, 145.0, 165.0, 185.0, 205.0],
        );
        assert!(adjusted.rect.x < original.rect.x);
        assert!(adjusted.rect.width > original.rect.width);

        let unchanged = adjust_text_box_for_normalization(&original, "進捗：1/1", "進捗：1/1", &[]);
        assert_eq!(unchanged.rect, original.rect);
    }

    #[test]
    fn shrinks_box_when_normalization_removes_timer_badge() {
        let original = TextBox {
            id: 9,
            rect: Rect::new(100.0, 20.0, 180.0, 24.0),
            confidence: 0.9,
            content_fingerprint: 9,
        };

        let adjusted = adjust_text_box_for_normalization(
            &original,
            "Q9日3時間",
            "9日3時間",
            &[125.0, 175.0, 205.0, 235.0, 265.0, 295.0],
        );

        assert_eq!(adjusted.rect.x, 160.0);
        assert_eq!(adjusted.rect.right(), original.rect.right());
        assert!(adjusted.rect.width < original.rect.width);
    }

    #[test]
    fn shrinks_box_when_normalization_removes_trailing_ui_value() {
        let original = TextBox {
            id: 10,
            rect: Rect::new(100.0, 20.0, 220.0, 24.0),
            confidence: 0.9,
            content_fingerprint: 10,
        };

        let adjusted = adjust_text_box_for_normalization(
            &original,
            "ダーニャ+25",
            "ダーニャ",
            &[120.0, 150.0, 180.0, 210.0, 240.0, 270.0, 300.0],
        );

        assert_eq!(adjusted.rect.x, original.rect.x);
        assert_eq!(adjusted.rect.right(), 225.0);
        assert!(adjusted.rect.width < original.rect.width);
    }

    #[test]
    fn shrinks_box_when_normalization_removes_screen_counter_and_leading_artifact() {
        let original = TextBox {
            id: 11,
            rect: Rect::new(100.0, 20.0, 260.0, 24.0),
            confidence: 0.9,
            content_fingerprint: 11,
        };

        let adjusted = adjust_text_box_for_normalization(
            &original,
            "Sリソース72/1000",
            "リソース",
            &[
                112.0, 140.0, 168.0, 196.0, 224.0, 252.0, 280.0, 308.0, 336.0, 364.0,
            ],
        );

        assert!(adjusted.rect.x > original.rect.x);
        assert_eq!(adjusted.rect.right(), 238.0);
        assert!(adjusted.rect.width < original.rect.width);
    }

    #[test]
    fn expands_box_when_normalization_inserts_trailing_punctuation() {
        let original = TextBox {
            id: 12,
            rect: Rect::new(100.0, 20.0, 80.0, 24.0),
            confidence: 0.9,
            content_fingerprint: 12,
        };

        let adjusted =
            adjust_text_box_for_normalization(&original, "15秒間持続", "15秒間持続。", &[]);

        assert_eq!(adjusted.rect.x, original.rect.x);
        assert!(adjusted.rect.right() > original.rect.right());
    }

    #[test]
    fn normalizes_ui_label_text() {
        assert_eq!(
            normalize_recognized_text("AppraisalLevel2"),
            "Appraisal Level 2"
        );
        assert_eq!(
            normalize_recognized_text("CityTvcoonCircleBounty"),
            "City Tvcoon Circle Bounty"
        );
        assert_eq!(
            normalize_recognized_text("UD：215213534277"),
            "UD：215213534277"
        );
        assert_eq!(
            normalize_recognized_text("ID：500055272"),
            "User ID:500055272"
        );
        assert_eq!(normalize_recognized_text("星声x50"), "星声x50");
        assert_eq!(
            normalize_recognized_text("シェルコイン×10000"),
            "シェルコインx10000"
        );
        assert_eq!(
            normalize_recognized_text("×0.12%分アップ"),
            "×0.12%分アップ"
        );
        assert_eq!(normalize_recognized_text("空想ショッフ"), "空想ショップ");
        assert_eq!(
            normalize_recognized_text("共鳴スギルダメージアッフ"),
            "共鳴スキルダメージアップ"
        );
        assert_eq!(normalize_recognized_text("持級チュナ"), "特級チュナ");
        assert_eq!(
            normalize_recognized_text("パラダイムシフトのすベて"),
            "パラダイムシフトのすべて"
        );
        assert_eq!(
            normalize_recognized_text("ハ一モ二一効果"),
            "ハーモニー効果"
        );
        assert_eq!(
            normalize_recognized_text("ハーモニーフィルターノすベて"),
            "ハーモニーフィルター/すべて"
        );
        assert_eq!(normalize_recognized_text("日標に2"), "目標に2");
        assert_eq!(
            normalize_recognized_text("持定商取引法に基つ"),
            "特定商取引法に基づ"
        );
        assert_eq!(
            normalize_recognized_text("協奏工ネルギー獲得"),
            "協奏エネルギー獲得"
        );
        assert_eq!(normalize_recognized_text("交换ショップ"), "交換ショップ");
        assert_eq!(normalize_recognized_text("壳り切れ"), "売り切れ");
        assert_eq!(
            normalize_recognized_text("共形エネルギー）"),
            "【共形エネルギー】"
        );
        assert_eq!(
            normalize_recognized_text("ヴォイドマター粒子"),
            "【ヴォイドマター粒子】"
        );
        assert_eq!(
            normalize_recognized_text("ウォイドマター粒子"),
            "【ヴォイドマター粒子】"
        );
        assert_eq!(
            normalize_recognized_text("キャラが敵に斉爆効果を付与"),
            "キャラが敵に【斉爆効果】を付与"
        );
        assert_eq!(
            normalize_recognized_text("砕けた記憶」または「砕けた悪夢」"),
            "「砕けた記憶」または「砕けた悪夢」"
        );
        assert_eq!(normalize_recognized_text("進：8/8"), "進捗：8/8");
        assert_eq!(normalize_recognized_text("進步：1/1"), "進捗：1/1");
        assert_eq!(normalize_recognized_text("准禁·1/1"), "進捗：1/1");
        assert_eq!(normalize_recognized_text("Q9日3時間"), "9日3時間");
        assert_eq!(normalize_recognized_text("G 9日3時間"), "9日3時間");
        assert_eq!(normalize_recognized_text("中音·銳…"), "中音・鋭...");
        assert_eq!(
            normalize_recognized_text("スタンプグッチョ!"),
            "[スタンプ]グッチョ！"
        );
        assert_eq!(
            normalize_recognized_text("響き渡る共鳴・ダーニャ+25"),
            "響き渡る共鳴・ダーニャ"
        );
        assert_eq!(
            normalize_recognized_text("共鳴解放ダメージアップ10.9%"),
            "共鳴解放ダメージアップ"
        );
        assert_eq!(normalize_recognized_text("I.R.I.S."), "I.R.I.S.");
        assert_eq!(normalize_recognized_text("B.1.N.G.O."), "B.1.N.G.O.");
        assert_eq!(
            normalize_recognized_text("空想の幻夢」をクリア"),
            "「空想の幻夢Ⅱ」をクリア"
        );
        assert_eq!(
            normalize_recognized_text("空想の幻夢V」をクリア"),
            "「空想の幻夢Ⅳ」をクリア"
        );
        assert_eq!(normalize_recognized_text("PTaptoquit"), "Tap to quit");
        assert_eq!(
            normalize_recognized_text("①Odetothe Second Sunrise"),
            "Ode to the Second Sunrise"
        );
        assert_eq!(
            normalize_recognized_text("Reachthe Reactor Coreandsave"),
            "Reach the Reactor Core and save"
        );
        assert_eq!(
            normalize_recognized_text("Jser ID:500055272"),
            "User ID:500055272"
        );
        assert_eq!(normalize_recognized_text("Rover:Havoc"), "Rover:Havoc");
        assert_eq!(
            normalize_recognized_text("住She'llbefine"),
            "She'll be fine"
        );
        assert_eq!(normalize_recognized_text("Mornye"), "Mornye");
        assert_eq!(normalize_recognized_text("Mornve"), "Mornye");
        assert_eq!(
            normalize_recognized_text("Commandunresponsive.Controlprogramfailure!"),
            "Command unresponsive. Control program failure!"
        );
        assert_eq!(
            normalize_recognized_text("Tellmewhat'shappening.We'll"),
            "Tell me what's happening. We'll"
        );
        assert_eq!(
            normalize_recognized_text("Youhaveanewmessage"),
            "You have a new message"
        );
        assert_eq!(
            normalize_recognized_text("Adjustthemeasurementlocation.No,iftheimpactisthis"),
            "Adjust the measurement location. No, if the impact is this"
        );
        assert_eq!(
            normalize_recognized_text("Tothe New World"),
            "To the New World"
        );
    }

    #[test]
    fn preserves_japanese_text() {
        // All-CJK/kana strings must survive normalization (regression: leading
        // noise trimming used to strip the entire string).
        assert_eq!(
            normalize_recognized_text("友達とご飯を食べました。"),
            "友達とご飯を食べました。"
        );
        assert_eq!(normalize_recognized_text("水を飲んだ。"), "水を飲んだ。");
        assert_eq!(
            normalize_recognized_text("高い山を見た。"),
            "高い山を見た。"
        );
        // Leading symbol noise is still trimmed, kana preserved.
        assert_eq!(normalize_recognized_text("①ラーメン"), "ラーメン");
        assert_eq!(normalize_recognized_text("受取济"), "受取済");
        assert_eq!(normalize_recognized_text("進埗：1/1"), "進捗：1/1");
        assert_eq!(normalize_recognized_text("攻擊力"), "攻撃力");
        assert_eq!(normalize_recognized_text("电磁効果"), "電磁効果");
        assert_eq!(normalize_recognized_text("共鸣解放"), "共鳴解放");
        assert_eq!(
            normalize_recognized_text("共鳴能力测定報告"),
            "共鳴能力測定報告"
        );
        assert_eq!(normalize_recognized_text("数值"), "数値");
    }

    #[test]
    fn fixes_kanji_katakana_lookalikes_between_katakana() {
        // 二(kanji) between katakana is really katakana ニ (the ハーモニー misread).
        assert_eq!(
            normalize_recognized_text("ハーモ二ー効果"),
            "ハーモニー効果"
        );
        assert_eq!(normalize_recognized_text("メ二ュー"), "メニュー");
        // But genuine kanji in kanji context is left alone.
        assert_eq!(normalize_recognized_text("第二章"), "第二章");
        assert_eq!(normalize_recognized_text("二人"), "二人");
        assert_eq!(normalize_recognized_text("攻撃力"), "攻撃力");
    }

    #[test]
    fn contrast_stretch_expands_low_contrast_range() {
        // Background grey 160 with faint grey 150 "text": delta of only 10.
        let mut image = RgbImage::from_pixel(20, 4, Rgb([160, 160, 160]));
        for x in 0..6 {
            image.put_pixel(x, 1, Rgb([150, 150, 150]));
        }
        let stretched = contrast_stretch(&image);

        let mut min = 255u8;
        let mut max = 0u8;
        for pixel in stretched.pixels() {
            let l = luminance(pixel);
            min = min.min(l);
            max = max.max(l);
        }
        assert!(
            max - min > 100,
            "expected stretched range to widen, got {min}..{max}"
        );
    }

    /// The integral-image formulation the sliding-window build replaced. Kept
    /// as the reference for the exact-equality test below.
    fn reference_local_contrast(image: &RgbImage) -> RgbImage {
        let width = image.width() as usize;
        let height = image.height() as usize;
        if width == 0 || height == 0 {
            return image.clone();
        }
        let luma = image.pixels().map(luminance).collect::<Vec<_>>();
        let stride = width + 1;
        let mut integral = vec![0u64; (width + 1) * (height + 1)];
        for y in 0..height {
            let mut row_sum = 0u64;
            for x in 0..width {
                row_sum += luma[y * width + x] as u64;
                integral[(y + 1) * stride + x + 1] = integral[y * stride + x + 1] + row_sum;
            }
        }
        let radius = LOCAL_CONTRAST_RADIUS_PX as usize;
        let mut out = vec![0u8; width * height * 3];
        for y in 0..height {
            for x in 0..width {
                let x0 = x.saturating_sub(radius);
                let y0 = y.saturating_sub(radius);
                let x1 = (x + radius + 1).min(width);
                let y1 = (y + radius + 1).min(height);
                let sum = integral[y1 * stride + x1] + integral[y0 * stride + x0]
                    - integral[y0 * stride + x1]
                    - integral[y1 * stride + x0];
                let count = ((x1 - x0) * (y1 - y0)).max(1);
                let local_mean = sum as f32 / count as f32;
                let value = 128.0 + (luma[y * width + x] as f32 - local_mean) * LOCAL_CONTRAST_GAIN;
                let value = value.round().clamp(0.0, 255.0) as u8;
                let offset = (y * width + x) * 3;
                out[offset] = value;
                out[offset + 1] = value;
                out[offset + 2] = value;
            }
        }
        RgbImage::from_raw(image.width(), image.height(), out)
            .expect("reference buffer matches dimensions")
    }

    #[test]
    fn sliding_window_local_contrast_matches_integral_reference_exactly() {
        // Deterministic but irregular pixel pattern; sizes chosen to exercise
        // width/height below, at, and above the window diameter (2*14+1 = 29),
        // plus chunk boundaries in the parallel vertical pass.
        let mut seed = 0x2545F491u32;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed
        };
        for (w, h) in [(1, 1), (5, 3), (29, 29), (64, 32), (100, 7), (31, 100)] {
            let mut image = RgbImage::new(w, h);
            for pixel in image.pixels_mut() {
                let v = next();
                *pixel = Rgb([v as u8, (v >> 8) as u8, (v >> 16) as u8]);
            }
            let fast = local_contrast_detector_image(&image);
            let reference = reference_local_contrast(&image);
            assert_eq!(
                fast.as_raw(),
                reference.as_raw(),
                "sliding-window output diverged from integral reference at {w}x{h}"
            );
        }
    }

    #[test]
    fn local_contrast_emphasizes_faint_glyphs() {
        let mut image = RgbImage::from_pixel(64, 32, Rgb([90, 90, 90]));
        for y in 10..22 {
            for x in 24..30 {
                image.put_pixel(x, y, Rgb([104, 104, 104]));
            }
        }

        let enhanced = local_contrast_detector_image(&image);
        let background = luminance(enhanced.get_pixel(8, 16));
        let glyph = luminance(enhanced.get_pixel(26, 16));

        assert!(
            glyph.saturating_sub(background) > 40,
            "expected local contrast to amplify faint glyphs, got background={background} glyph={glyph}"
        );
    }

    #[test]
    fn merge_detection_rects_dedupes_overlapping_passes() {
        let config = PipelineConfig::default();
        let frame_size = Size::new(400, 200);
        let candidates = vec![
            ScoredRect {
                rect: Rect::new(20.0, 30.0, 100.0, 24.0),
                score: 0.90,
                pass: DetectorPass::Primary,
            },
            ScoredRect {
                rect: Rect::new(22.0, 29.0, 104.0, 26.0),
                score: 0.70,
                pass: DetectorPass::LowContrast,
            },
            ScoredRect {
                rect: Rect::new(260.0, 120.0, 80.0, 20.0),
                score: 0.80,
                pass: DetectorPass::LowContrast,
            },
        ];

        let merged = merge_detection_rects(candidates, frame_size, &config);

        assert_eq!(merged.len(), 2);
        assert!(merged.iter().any(|candidate| {
            candidate.rect == Rect::new(20.0, 30.0, 100.0, 24.0)
                && candidate.score == 0.90
                && candidate.pass == DetectorPass::Primary
        }));
    }

    #[test]
    fn area_downscale_averages_2x2_block() {
        // 2x2 grayscale image → 1x1 must be the mean of all four pixels.
        let mut image = RgbImage::new(2, 2);
        image.put_pixel(0, 0, Rgb([10, 10, 10]));
        image.put_pixel(1, 0, Rgb([20, 20, 20]));
        image.put_pixel(0, 1, Rgb([30, 30, 30]));
        image.put_pixel(1, 1, Rgb([40, 40, 40]));
        let out = area_downscale(&image, 1, 1);
        assert_eq!(out.dimensions(), (1, 1));
        // mean(10,20,30,40) = 25.
        assert_eq!(out.get_pixel(0, 0), &Rgb([25, 25, 25]));
    }

    #[test]
    fn area_downscale_preserves_a_uniform_field() {
        // A flat color must survive any downscale unchanged (coverage weights
        // sum to 1), with the requested output dimensions.
        let image = RgbImage::from_pixel(64, 40, Rgb([123, 45, 67]));
        let out = area_downscale(&image, 20, 13);
        assert_eq!(out.dimensions(), (20, 13));
        for pixel in out.pixels() {
            assert_eq!(pixel, &Rgb([123, 45, 67]));
        }
    }

    #[test]
    fn resize_for_detector_pads_native_input_instead_of_resampling() {
        let mut image = RgbImage::from_pixel(640, 625, Rgb([10, 20, 30]));
        image.put_pixel(639, 624, Rgb([200, 210, 220]));

        let out = resize_for_detector(&image, 640, ResizeFilter::CatmullRom);

        assert_eq!(out.image.dimensions(), (640, 625));
        assert_eq!(out.content_size(), Size::new(640, 625));
        assert_eq!(out.padded_size, Size::new(640, 640));
        assert_eq!(out.image.get_pixel(639, 624), &Rgb([200, 210, 220]));
    }

    #[test]
    fn resize_for_detector_resizes_when_alignment_padding_is_material() {
        let image = RgbImage::from_pixel(64, 33, Rgb([10, 20, 30]));

        let out = resize_for_detector(&image, 64, ResizeFilter::CatmullRom);

        assert_eq!(out.image.dimensions(), (64, 64));
        assert_eq!(out.content_size(), Size::new(64, 64));
        assert_eq!(out.padded_size, Size::new(64, 64));
    }

    #[test]
    fn detector_tensor_zero_pads_detector_input() {
        let mut image = RgbImage::from_pixel(640, 625, Rgb([10, 20, 30]));
        image.put_pixel(639, 624, Rgb([200, 210, 220]));
        let input = resize_for_detector(&image, 640, ResizeFilter::CatmullRom);

        let tensor = detector_tensor(&input);
        let width = input.padded_size.width as usize;
        let plane = width * input.padded_size.height as usize;
        let content_index = 624 * width + 639;
        let padding_index = 625 * width + 639;

        assert_ne!(tensor[content_index], 0.0);
        for channel in 0..DET_CHANNELS {
            assert_eq!(tensor[channel * plane + padding_index], 0.0);
        }
    }

    #[test]
    fn padded_detector_scaling_ignores_padding_extent() {
        let rect = Rect::new(10.0, 32.0, 20.0, 1.0);
        let scaled = scale_detector_map_rect_to_frame(
            rect,
            Size::new(64, 64),
            Size::new(64, 33),
            Size::new(64, 64),
            Size::new(640, 330),
        )
        .expect("content rect should survive");

        assert_eq!(scaled, Rect::new(100.0, 320.0, 200.0, 10.0));

        let padding_rect = Rect::new(0.0, 40.0, 64.0, 10.0);
        assert!(
            scale_detector_map_rect_to_frame(
                padding_rect,
                Size::new(64, 64),
                Size::new(64, 33),
                Size::new(64, 64),
                Size::new(640, 330),
            )
            .is_none()
        );
    }

    #[test]
    fn area_downscale_matches_horizontal_gradient_means() {
        // 4×1 ramp 0,80,160,240 downscaled to 2×1: each output pixel is the mean
        // of its two source columns → (0+80)/2=40, (160+240)/2=200.
        let mut image = RgbImage::new(4, 1);
        for (x, v) in [0u8, 80, 160, 240].into_iter().enumerate() {
            image.put_pixel(x as u32, 0, Rgb([v, v, v]));
        }
        let out = area_downscale(&image, 2, 1);
        assert_eq!(out.get_pixel(0, 0), &Rgb([40, 40, 40]));
        assert_eq!(out.get_pixel(1, 0), &Rgb([200, 200, 200]));
    }

    #[test]
    fn trt_profile_shapes_are_well_formed_and_bracketed() {
        // Recognizer profile: fixed height 48, batch up to REC_BATCH_MAX, width to
        // 960; min ≤ opt ≤ max on every dynamic dim so one engine covers the range.
        let (min, opt, max) = providers::trt_profile_shapes(ModelKind::Recognizer);
        assert_eq!(min, "x:1x3x48x16");
        assert_eq!(opt, format!("x:{REC_BATCH_MAX}x3x48x320"));
        assert_eq!(max, format!("x:{REC_BATCH_MAX}x3x48x960"));

        // Detector profile is single-image (batch 1), 3-channel, square-bounded.
        let (dmin, dopt, dmax) = providers::trt_profile_shapes(ModelKind::Detector);
        for s in [&dmin, &dopt, &dmax] {
            assert!(s.starts_with("x:1x3x"), "unexpected detector profile {s}");
        }
    }
}
