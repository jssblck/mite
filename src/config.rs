use std::fs;
use std::ops::Deref;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize)]
pub struct AppConfig {
    pub runtime: RuntimeConfig,
    pub models: ModelConfig,
    pub pipeline: CheckedPipelineConfig,
    pub overlay: OverlayConfig,
}

impl AppConfig {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let raw = fs::read_to_string(path)
            .with_context(|| format!("failed to read config {}", path.display()))?;
        Self::parse_toml(&raw).with_context(|| format!("invalid config {}", path.display()))
    }

    pub fn parse_toml(raw: &str) -> Result<Self> {
        let raw_config: RawAppConfig = toml::from_str(raw).context("failed to parse config")?;
        raw_config.parse()
    }

    pub fn write_default(path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        let text = toml::to_string_pretty(&Self::default())?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        fs::write(path, text).with_context(|| format!("failed to write {}", path.display()))
    }
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default)]
struct RawAppConfig {
    runtime: RuntimeConfig,
    models: ModelConfig,
    pipeline: PipelineConfig,
    overlay: OverlayConfig,
}

impl RawAppConfig {
    fn parse(self) -> Result<AppConfig> {
        Ok(AppConfig {
            runtime: self.runtime,
            models: self.models,
            pipeline: self.pipeline.parse().context("invalid pipeline config")?,
            overlay: self.overlay,
        })
    }
}

/// Image resampling filter for detector downscaling. Maps to
/// `image::imageops::FilterType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResizeFilter {
    /// Nearest-neighbour: fastest, aliases badly (debug only).
    Nearest,
    /// Bilinear: fast, mild blur.
    Triangle,
    /// Bicubic (Catmull-Rom): sharper than bilinear, little ringing.
    CatmullRom,
    /// Windowed sinc: sharpest detail retention, some ringing.
    Lanczos3,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RuntimeConfig {
    pub backend: RuntimeBackend,
    pub fp16: bool,
    /// Run the explicit-QDQ INT8 detector variant (the `-int8` sibling of the
    /// configured detector path, produced by `scripts/quantize-models.py`).
    /// TensorRT builds mixed INT8/FP16 engines from it, picking the fastest
    /// precision per layer.
    pub int8_detector: bool,
    /// Same as `int8_detector`, for the primary recognizer. The optional
    /// fallback recognizer is unaffected (it always runs FP32).
    pub int8_recognizer: bool,
    pub engine_cache_dir: PathBuf,
}

impl RuntimeConfig {
    /// Whether `kind_is_detector` selects an INT8 model under this runtime.
    pub fn int8_for(&self, detector: bool) -> bool {
        if detector {
            self.int8_detector
        } else {
            self.int8_recognizer
        }
    }
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            backend: RuntimeBackend::NvidiaTensorRtThenCuda,
            fp16: true,
            int8_detector: false,
            int8_recognizer: false,
            engine_cache_dir: PathBuf::from("cache/engines"),
        }
    }
}

/// The `-int8` sibling of a model path: `models/foo.onnx` ->
/// `models/foo-int8.onnx`.
pub fn int8_model_path(path: &Path) -> PathBuf {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    let extension = path.extension().and_then(|s| s.to_str()).unwrap_or("onnx");
    path.with_file_name(format!("{stem}-int8.{extension}"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeBackend {
    Fixture,
    NvidiaTensorRtThenCuda,
    Cuda,
    DirectMl,
    OpenVino,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelConfig {
    pub detector_path: PathBuf,
    pub recognizer_path: PathBuf,
    pub charset_path: Option<PathBuf>,
    /// Optional heavier recognizer used as a second opinion on lines the
    /// primary recognizer reads with low confidence (faint or stylized text).
    /// Loaded in FP32 (the PP-OCRv5 server recognizer overflows FP16) and only
    /// consulted for the few low-confidence lines per frame, so the GPU cost
    /// stays small. Its read replaces the primary's only when it is both
    /// confident in absolute terms and more confident than the primary.
    pub fallback_recognizer_path: Option<PathBuf>,
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            detector_path: PathBuf::from("models/pp-ocrv5-mobile-det.onnx"),
            recognizer_path: PathBuf::from("models/pp-ocrv5-mobile-rec.onnx"),
            charset_path: Some(PathBuf::from("models/pp-ocrv5-dict.txt")),
            fallback_recognizer_path: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PipelineConfig {
    /// Fraction of a frame's native long side fed to the detector when the frame
    /// is larger than `detector_min_long_side`. 1.0 = native resolution up to
    /// `detector_max_long_side`; 0.5 = half resolution.
    pub detector_downscale: f32,
    /// Floor for the detector's input long side: frames at or below this run at
    /// native resolution (no downscale), and downscaling never produces a smaller
    /// long side than this.
    pub detector_min_long_side: u32,
    /// Hard ceiling on the detector's input long side, to bound cost on very
    /// high-resolution displays.
    pub detector_max_long_side: u32,
    /// Enlarge the detector input beyond native resolution by this factor
    /// (1.0 = native). Small isolated glyphs the detector misses at native
    /// resolution become detectable at 1.5x, at significant inference cost.
    pub detector_upscale: f32,
    /// Resampling filter used when scaling the frame for the detector. Higher
    /// quality filters retain more fine text detail when downscaling, at some CPU
    /// cost (paid once per frame).
    pub detector_resize_filter: ResizeFilter,
    pub detector_cadence_frames: u64,
    pub max_boxes_per_frame: usize,
    pub recognition_cache_ttl_frames: u64,
    pub stale_frame_budget: usize,
    pub detector_probability_threshold: f32,
    pub detector_box_score_threshold: f32,
    /// Run an extra detector pass on a local-contrast luminance view. This costs
    /// another detector inference, but recovers faint UI text that the standard
    /// detector input misses.
    pub detector_low_contrast_pass: bool,
    pub detector_low_contrast_probability_threshold: f32,
    pub detector_low_contrast_box_score_threshold: f32,
    pub detector_min_component_area: usize,
    /// Horizontal binary-close radius, in detector-map pixels, applied before
    /// connected-component extraction. This bridges small gaps between glyph
    /// strokes without invoking a whole low-contrast detector pass.
    pub detector_close_radius_px: u32,
    /// DB-style unclipping ratio applied to each detector component rectangle.
    /// `0.0` disables expansion; higher values recover margins around text that
    /// the detector probability map marks too tightly.
    pub detector_unclip_ratio: f32,
    pub detector_max_box_height_ratio: f32,
    pub detector_max_box_area_ratio: f32,
    pub min_recognition_confidence: f32,
    pub min_single_character_confidence: f32,
    /// Apply a per-image min/max luminance contrast stretch before detection and
    /// recognition. Off by default to preserve the game-tuned pipeline; recovers
    /// very low-contrast text at a measurable accuracy/latency tradeoff.
    pub detector_contrast_stretch: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(transparent)]
pub struct CheckedPipelineConfig(PipelineConfig);

impl CheckedPipelineConfig {
    pub fn get(&self) -> &PipelineConfig {
        &self.0
    }

    pub fn into_inner(self) -> PipelineConfig {
        self.0
    }
}

impl Deref for CheckedPipelineConfig {
    type Target = PipelineConfig;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Default for CheckedPipelineConfig {
    fn default() -> Self {
        PipelineConfig::default()
            .parse()
            .expect("default pipeline config must parse")
    }
}

impl PipelineConfig {
    /// Reject values that are nonsensical or would only fail deep in the
    /// pipeline: fractions outside `[0, 1]`, a zero downscale, or a min long
    /// side above the max. Returns a checked wrapper so callers preserve that
    /// proof instead of discarding it.
    pub fn parse(self) -> Result<CheckedPipelineConfig> {
        for (name, value) in [
            (
                "detector_probability_threshold",
                self.detector_probability_threshold,
            ),
            (
                "detector_box_score_threshold",
                self.detector_box_score_threshold,
            ),
            (
                "detector_low_contrast_probability_threshold",
                self.detector_low_contrast_probability_threshold,
            ),
            (
                "detector_low_contrast_box_score_threshold",
                self.detector_low_contrast_box_score_threshold,
            ),
            (
                "detector_max_box_height_ratio",
                self.detector_max_box_height_ratio,
            ),
            (
                "detector_max_box_area_ratio",
                self.detector_max_box_area_ratio,
            ),
            (
                "min_recognition_confidence",
                self.min_recognition_confidence,
            ),
            (
                "min_single_character_confidence",
                self.min_single_character_confidence,
            ),
        ] {
            if !(0.0..=1.0).contains(&value) {
                bail!("{name} must be in [0, 1], got {value}");
            }
        }
        if !(0.0..=1.0).contains(&self.detector_downscale) || self.detector_downscale == 0.0 {
            bail!(
                "detector_downscale must be in (0, 1], got {}",
                self.detector_downscale
            );
        }
        if !(1.0..=2.0).contains(&self.detector_upscale) {
            bail!(
                "detector_upscale must be in [1, 2], got {}",
                self.detector_upscale
            );
        }
        if self.detector_min_long_side > self.detector_max_long_side {
            bail!(
                "detector_min_long_side ({}) must be <= detector_max_long_side ({})",
                self.detector_min_long_side,
                self.detector_max_long_side
            );
        }
        if !(0.0..=5.0).contains(&self.detector_unclip_ratio) {
            bail!(
                "detector_unclip_ratio must be in [0, 5], got {}",
                self.detector_unclip_ratio
            );
        }
        Ok(CheckedPipelineConfig(self))
    }

    /// Detector input long side for a frame whose native long side is `native`.
    /// Scales by `detector_upscale` (above 1.0 the frame is enlarged so the
    /// detector sees small glyphs at more pixels), then applies
    /// `detector_downscale`, clamped to the configured min/max long sides.
    pub fn detector_target_long_side(&self, native: u32) -> u32 {
        let upscaled = (native as f32 * self.detector_upscale.max(1.0)).round() as u32;
        let target = if upscaled <= self.detector_min_long_side {
            upscaled
        } else {
            let scaled = (upscaled as f32 * self.detector_downscale).round() as u32;
            scaled.max(self.detector_min_long_side)
        };
        target.min(self.detector_max_long_side).max(1)
    }
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            detector_downscale: 1.0,
            detector_min_long_side: 3840,
            detector_max_long_side: 3840,
            detector_upscale: 1.0,
            detector_resize_filter: ResizeFilter::CatmullRom,
            detector_cadence_frames: 2,
            // Dense 4K menu screens really do contain 80-100+ text lines;
            // boxes beyond the cap are silently dropped, which the eval corpus
            // showed as whole missing panels. 256 is a safety valve, not a
            // typical load.
            max_boxes_per_frame: 256,
            recognition_cache_ttl_frames: 45,
            stale_frame_budget: 1,
            detector_probability_threshold: 0.30,
            detector_box_score_threshold: 0.45,
            detector_low_contrast_pass: true,
            detector_low_contrast_probability_threshold: 0.20,
            detector_low_contrast_box_score_threshold: 0.25,
            detector_min_component_area: 8,
            detector_close_radius_px: 0,
            detector_unclip_ratio: 0.0,
            detector_max_box_height_ratio: 0.08,
            detector_max_box_area_ratio: 0.02,
            min_recognition_confidence: 0.50,
            min_single_character_confidence: 0.85,
            detector_contrast_stretch: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct OverlayConfig {
    pub enabled: bool,
    pub click_through: bool,
    pub show_confidence: bool,
}

impl Default for OverlayConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            click_through: true,
            show_confidence: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn int8_model_path_derives_sibling() {
        assert_eq!(
            int8_model_path(Path::new("models/pp-ocrv5-mobile-det.onnx")),
            PathBuf::from("models/pp-ocrv5-mobile-det-int8.onnx")
        );
    }

    #[test]
    fn default_config_round_trips() {
        let text = toml::to_string_pretty(&AppConfig::default()).unwrap();
        let decoded = AppConfig::parse_toml(&text).unwrap();
        assert_eq!(
            decoded.runtime.backend,
            RuntimeBackend::NvidiaTensorRtThenCuda
        );
        assert_eq!(decoded.pipeline.detector_min_long_side, 3840);
        assert_eq!(decoded.pipeline.detector_downscale, 1.0);
    }

    #[test]
    fn load_rejects_out_of_range_pipeline_values() {
        assert!(PipelineConfig::default().parse().is_ok());

        let bad_ratio = PipelineConfig {
            min_recognition_confidence: 1.5,
            ..PipelineConfig::default()
        };
        assert!(bad_ratio.parse().is_err());

        let inverted = PipelineConfig {
            detector_min_long_side: 4000,
            detector_max_long_side: 1920,
            ..PipelineConfig::default()
        };
        assert!(inverted.parse().is_err());

        let zero_downscale = PipelineConfig {
            detector_downscale: 0.0,
            ..PipelineConfig::default()
        };
        assert!(zero_downscale.parse().is_err());
    }

    #[test]
    fn detector_target_long_side_policy() {
        let cfg = PipelineConfig::default();
        // 4K and below now run native by default.
        assert_eq!(cfg.detector_target_long_side(3840), 3840);
        assert_eq!(cfg.detector_target_long_side(1920), 1920);
        assert_eq!(cfg.detector_target_long_side(1280), 1280);
        // Above 4K is capped to bound cost.
        assert_eq!(cfg.detector_target_long_side(5120), 3840);
        assert_eq!(cfg.detector_target_long_side(7680), 3840);

        let fast = PipelineConfig {
            detector_downscale: 0.5,
            detector_min_long_side: 1920,
            detector_max_long_side: 1920,
            ..PipelineConfig::default()
        };
        assert_eq!(fast.detector_target_long_side(3840), 1920);
        assert_eq!(fast.detector_target_long_side(2560), 1920);
    }
}
