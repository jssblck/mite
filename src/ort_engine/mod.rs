use std::borrow::Cow;
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
    dictionary: Vec<String>,
    stable_ids: StableIdAllocator,
    /// Set from the pipeline config during `detect`, reused by `recognize`
    /// (which has no config argument) so both stages see the same preprocessing.
    contrast_stretch: bool,
}

impl OrtOcrEngine {
    pub fn new(models: &ModelConfig, runtime: &RuntimeConfig) -> Result<Self> {
        require_file(&models.detector_path)?;
        require_file(&models.recognizer_path)?;
        let charset_path = models
            .charset_path
            .as_ref()
            .context("models.charset_path is required for real OCR")?;
        require_file(charset_path)?;

        let detector = commit_session(&models.detector_path, runtime, ModelKind::Detector)?;
        let recognizer = commit_session(&models.recognizer_path, runtime, ModelKind::Recognizer)?;
        let dictionary = fs::read_to_string(charset_path)
            .with_context(|| format!("failed to read {}", charset_path.display()))?
            .lines()
            .map(str::to_string)
            .collect::<Vec<_>>();

        Ok(Self {
            detector,
            recognizer,
            dictionary,
            stable_ids: StableIdAllocator::default(),
            contrast_stretch: false,
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
        let image = maybe_contrast_stretch(image, self.contrast_stretch);
        let native_long = image.width().max(image.height());
        let target_long = config.detector_target_long_side(native_long);
        let detector_input = prof("det.resize", || {
            resize_for_detector(image.as_ref(), target_long, config.detector_resize_filter)
        });
        let mut candidates = prof("det.primary", || {
            detect_candidate_rects(
                &mut self.detector,
                &detector_input,
                frame.size,
                config,
                DetectorPassConfig::primary(config),
            )
        })?;

        if config.detector_low_contrast_pass {
            let enhanced = prof("det.local_contrast", || {
                local_contrast_detector_image(detector_input.image.as_ref())
            });
            let enhanced_input = DetectorInput {
                image: Cow::Owned(enhanced),
                padded_size: detector_input.padded_size,
            };
            candidates.extend(prof("det.low_contrast", || {
                detect_candidate_rects(
                    &mut self.detector,
                    &enhanced_input,
                    frame.size,
                    config,
                    DetectorPassConfig::low_contrast(config),
                )
            })?);
        }

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
                    let crop = crop_text_line(frame_image, text_box.rect)?;
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
        prof("rec.batches", || -> Result<()> {
            for chunk in order.chunks(REC_BATCH_MAX) {
                run_recognizer_batch(
                    &mut self.recognizer,
                    &self.dictionary,
                    &crops,
                    chunk,
                    boxes,
                    &mut results,
                )?;
            }
            Ok(())
        })?;

        Ok(results
            .into_iter()
            .map(|result| result.expect("every box produces a recognition result"))
            .collect())
    }
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
/// strokes on translucent panels and grey-on-grey game UI.
fn local_contrast_detector_image(image: &RgbImage) -> RgbImage {
    let width = image.width() as usize;
    let height = image.height() as usize;
    if width == 0 || height == 0 {
        return image.clone();
    }

    let luma = image.pixels().map(luminance).collect::<Vec<_>>();
    let integral = luminance_integral(&luma, width, height);
    let radius = LOCAL_CONTRAST_RADIUS_PX as usize;
    let mut out = vec![0u8; width * height * 3];

    out.par_chunks_mut(width * 3)
        .enumerate()
        .for_each(|(y, row)| {
            for x in 0..width {
                let local_mean = local_mean_luma(&integral, width, height, x, y, radius);
                let value = 128.0 + (luma[y * width + x] as f32 - local_mean) * LOCAL_CONTRAST_GAIN;
                let value = value.round().clamp(0.0, 255.0) as u8;
                let offset = x * 3;
                row[offset] = value;
                row[offset + 1] = value;
                row[offset + 2] = value;
            }
        });

    RgbImage::from_raw(image.width(), image.height(), out)
        .expect("local contrast buffer matches dimensions")
}

fn luminance_integral(luma: &[u8], width: usize, height: usize) -> Vec<u64> {
    let stride = width + 1;
    let mut integral = vec![0u64; (width + 1) * (height + 1)];
    for y in 0..height {
        let mut row_sum = 0u64;
        for x in 0..width {
            row_sum += luma[y * width + x] as u64;
            integral[(y + 1) * stride + x + 1] = integral[y * stride + x + 1] + row_sum;
        }
    }
    integral
}

fn local_mean_luma(
    integral: &[u64],
    width: usize,
    height: usize,
    x: usize,
    y: usize,
    radius: usize,
) -> f32 {
    let stride = width + 1;
    let x0 = x.saturating_sub(radius);
    let y0 = y.saturating_sub(radius);
    let x1 = (x + radius + 1).min(width);
    let y1 = (y + radius + 1).min(height);
    let sum = integral[y1 * stride + x1] + integral[y0 * stride + x0]
        - integral[y0 * stride + x1]
        - integral[y1 * stride + x0];
    let count = ((x1 - x0) * (y1 - y0)).max(1);
    sum as f32 / count as f32
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
    pass: DetectorPass,
    label: &'static str,
}

impl DetectorPassConfig {
    fn primary(config: &PipelineConfig) -> Self {
        Self {
            probability_threshold: config.detector_probability_threshold,
            box_score_threshold: config.detector_box_score_threshold,
            min_component_area: config.detector_min_component_area,
            pass: DetectorPass::Primary,
            label: "det.primary",
        }
    }

    fn low_contrast(config: &PipelineConfig) -> Self {
        Self {
            probability_threshold: config.detector_low_contrast_probability_threshold,
            box_score_threshold: config.detector_low_contrast_box_score_threshold,
            min_component_area: config.detector_min_component_area,
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
    let (shape, data) = outputs[0].try_extract_tensor::<f32>()?;
    let dims = shape
        .iter()
        .map(|dim| usize::try_from(*dim).unwrap_or_default())
        .collect::<Vec<_>>();
    if dims.len() != 4 || dims[0] != 1 || dims[1] != 1 {
        bail!("unexpected detector output shape: {dims:?}");
    }

    let map_h = dims[2];
    let map_w = dims[3];
    let mut candidates = prof(&format!("{label}.components"), || {
        detect_components_from_probability_map(
            data,
            map_w,
            map_h,
            pass_config.probability_threshold,
            pass_config.min_component_area,
        )
    });
    candidates.sort_by(|a, b| {
        b.score()
            .total_cmp(&a.score())
            .then_with(|| a.rect.y.total_cmp(&b.rect.y))
            .then_with(|| a.rect.x.total_cmp(&b.rect.x))
            .then_with(|| a.rect.width.total_cmp(&b.rect.width))
            .then_with(|| a.rect.height.total_cmp(&b.rect.height))
    });

    let input_size = input_image.padded_size;
    let map_size = Size::new(
        u32::try_from(map_w).unwrap_or(u32::MAX),
        u32::try_from(map_h).unwrap_or(u32::MAX),
    );
    Ok(candidates
        .into_iter()
        .filter(|candidate| candidate.score() >= pass_config.box_score_threshold)
        .filter_map(|candidate| {
            let rect = scale_detector_map_rect_to_frame(
                candidate.rect,
                input_size,
                input_image.content_size(),
                map_size,
                frame_size,
            )?;
            Some(ScoredRect {
                rect,
                score: candidate.score(),
                pass: pass_config.pass,
            })
        })
        .filter(|candidate| is_plausible_text_rect(candidate.rect, frame_size, config))
        .collect())
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
        .take(config.max_boxes_per_frame)
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
    let input = Tensor::from_array((
        [rows, REC_CHANNELS, height, target_w],
        tensor.into_boxed_slice(),
    ))?;
    let outputs = prof(&format!("  rec.infer[n={rows} w={target_w}]"), || {
        recognizer.run(ort::inputs![input])
    })?;
    let (shape, data) = outputs[0].try_extract_tensor::<f32>()?;
    let dims = shape
        .iter()
        .map(|dim| usize::try_from(*dim).unwrap_or_default())
        .collect::<Vec<_>>();
    if dims.len() != 3 || dims[0] != rows {
        bail!("unexpected recognizer output shape: {dims:?} (expected batch {rows})");
    }
    let timesteps = dims[1];
    let classes = dims[2];

    // CTC-decode every row in parallel: the per-timestep argmax over the large
    // recognizer charset is a meaningful CPU cost, and rows are independent.
    let decoded: Vec<(usize, RecognizedText)> = batch
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
            (
                crop.index,
                RecognizedText {
                    text_box: boxes[crop.index].clone(),
                    text,
                    confidence,
                    reused: false,
                    char_centers,
                },
            )
        })
        .collect();
    for (index, recognized) in decoded {
        results[index] = Some(recognized);
    }
    Ok(())
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

fn detect_components_from_probability_map(
    data: &[f32],
    width: usize,
    height: usize,
    threshold: f32,
    min_component_area: usize,
) -> Vec<DetectionCandidate> {
    let mut visited = vec![false; width * height];
    let mut candidates = Vec::new();
    let min_area = min_component_area.max(1);

    for y in 0..height {
        for x in 0..width {
            let index = y * width + x;
            if visited[index] || data[index] < threshold {
                continue;
            }

            let mut stack = vec![(x, y)];
            visited[index] = true;
            let mut min_x = x;
            let mut max_x = x;
            let mut min_y = y;
            let mut max_y = y;
            let mut area = 0usize;
            let mut score_sum = 0.0f32;

            while let Some((cx, cy)) = stack.pop() {
                area += 1;
                score_sum += data[cy * width + cx].clamp(0.0, 1.0);
                min_x = min_x.min(cx);
                max_x = max_x.max(cx);
                min_y = min_y.min(cy);
                max_y = max_y.max(cy);

                let neighbors = [
                    (cx.wrapping_sub(1), cy, cx > 0),
                    (cx + 1, cy, cx + 1 < width),
                    (cx, cy.wrapping_sub(1), cy > 0),
                    (cx, cy + 1, cy + 1 < height),
                ];
                for (nx, ny, valid) in neighbors {
                    if !valid {
                        continue;
                    }
                    let next_index = ny * width + nx;
                    if !visited[next_index] && data[next_index] >= threshold {
                        visited[next_index] = true;
                        stack.push((nx, ny));
                    }
                }
            }

            if area >= min_area {
                candidates.push(DetectionCandidate {
                    rect: Rect::new(
                        min_x as f32,
                        min_y as f32,
                        (max_x - min_x + 1) as f32,
                        (max_y - min_y + 1) as f32,
                    ),
                    area,
                    score_sum,
                });
            }
        }
    }

    candidates
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

/// Padding around the detected rect, as a fraction of its height (with a floor).
const CROP_PAD_RATIO: f32 = 0.30;
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

fn crop_text_line(image: &RgbImage, rect: Rect) -> Result<RgbImage> {
    let bounds = crop_bounds(image.width(), image.height(), rect);
    Ok(imageops::crop_imm(image, bounds.x, bounds.y, bounds.width, bounds.height).to_image())
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
        '：' => Some(':'),
        '／' => Some('/'),
        '－' => Some('-'),
        '（' => Some('('),
        '）' => Some(')'),
        '济' => Some('済'),
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
        );
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].rect, Rect::new(3.0, 2.0, 4.0, 3.0));
        assert!((candidates[0].score() - 0.9).abs() < 0.001);
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
        // Full-width colon in raw maps to ASCII ':' in normalized (1:1).
        let raw = "A：B";
        let centers = [10.0, 20.0, 30.0];
        let aligned = align_centers(raw, &centers, "A:B");
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
            "UD:215213534277"
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
