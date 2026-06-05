use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub runtime: RuntimeConfig,
    pub models: ModelConfig,
    pub pipeline: PipelineConfig,
    pub overlay: OverlayConfig,
}

impl AppConfig {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let raw = fs::read_to_string(path)
            .with_context(|| format!("failed to read config {}", path.display()))?;
        let config: AppConfig = toml::from_str(&raw)
            .with_context(|| format!("failed to parse config {}", path.display()))?;
        config
            .pipeline
            .validate()
            .with_context(|| format!("invalid pipeline config in {}", path.display()))?;
        Ok(config)
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
    pub engine_cache_dir: PathBuf,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            backend: RuntimeBackend::NvidiaTensorRtThenCuda,
            fp16: true,
            engine_cache_dir: PathBuf::from("cache/engines"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeBackend {
    Mock,
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
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            detector_path: PathBuf::from("models/pp-ocrv5-mobile-det.onnx"),
            recognizer_path: PathBuf::from("models/pp-ocrv5-mobile-rec.onnx"),
            charset_path: Some(PathBuf::from("models/pp-ocrv5-dict.txt")),
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
    pub detector_max_box_height_ratio: f32,
    pub detector_max_box_area_ratio: f32,
    pub min_recognition_confidence: f32,
    pub min_single_character_confidence: f32,
    /// Apply a per-image min/max luminance contrast stretch before detection and
    /// recognition. Off by default to preserve the game-tuned pipeline; recovers
    /// very low-contrast text at a measurable accuracy/latency tradeoff.
    pub detector_contrast_stretch: bool,
}

impl PipelineConfig {
    /// Reject values that are nonsensical or would only fail deep in the
    /// pipeline: fractions outside `[0, 1]`, a zero downscale, or a min long
    /// side above the max. Called by [`AppConfig::load`] so a bad config fails
    /// fast at the boundary with a clear message.
    pub fn validate(&self) -> Result<()> {
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
        if self.detector_min_long_side > self.detector_max_long_side {
            bail!(
                "detector_min_long_side ({}) must be <= detector_max_long_side ({})",
                self.detector_min_long_side,
                self.detector_max_long_side
            );
        }
        Ok(())
    }

    /// Detector input long side for a frame whose native long side is `native`.
    /// Downscales to `detector_downscale` of native, but never below
    /// `detector_min_long_side`, and never above `detector_max_long_side`.
    pub fn detector_target_long_side(&self, native: u32) -> u32 {
        let target = if native <= self.detector_min_long_side {
            native
        } else {
            let scaled = (native as f32 * self.detector_downscale).round() as u32;
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
            detector_resize_filter: ResizeFilter::CatmullRom,
            detector_cadence_frames: 2,
            max_boxes_per_frame: 64,
            recognition_cache_ttl_frames: 45,
            stale_frame_budget: 1,
            detector_probability_threshold: 0.30,
            detector_box_score_threshold: 0.45,
            detector_low_contrast_pass: false,
            detector_low_contrast_probability_threshold: 0.20,
            detector_low_contrast_box_score_threshold: 0.25,
            detector_min_component_area: 8,
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
    fn default_config_round_trips() {
        let text = toml::to_string_pretty(&AppConfig::default()).unwrap();
        let decoded: AppConfig = toml::from_str(&text).unwrap();
        assert_eq!(
            decoded.runtime.backend,
            RuntimeBackend::NvidiaTensorRtThenCuda
        );
        assert_eq!(decoded.pipeline.detector_min_long_side, 3840);
        assert_eq!(decoded.pipeline.detector_downscale, 1.0);
    }

    #[test]
    fn load_rejects_out_of_range_pipeline_values() {
        assert!(PipelineConfig::default().validate().is_ok());

        let bad_ratio = PipelineConfig {
            min_recognition_confidence: 1.5,
            ..PipelineConfig::default()
        };
        assert!(bad_ratio.validate().is_err());

        let inverted = PipelineConfig {
            detector_min_long_side: 4000,
            detector_max_long_side: 1920,
            ..PipelineConfig::default()
        };
        assert!(inverted.validate().is_err());

        let zero_downscale = PipelineConfig {
            detector_downscale: 0.0,
            ..PipelineConfig::default()
        };
        assert!(zero_downscale.validate().is_err());
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
