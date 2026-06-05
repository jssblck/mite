use std::collections::HashMap;

use anyhow::Result;
use serde::Serialize;

use crate::capture::Frame;
use crate::config::{ModelConfig, PipelineConfig, RuntimeBackend, RuntimeConfig};
use crate::geometry::{Rect, sort_reading_order};

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TextBox {
    pub id: u64,
    pub rect: Rect,
    pub confidence: f32,
    pub content_fingerprint: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RecognizedText {
    pub text_box: TextBox,
    pub text: String,
    pub confidence: f32,
    pub reused: bool,
    /// Frame-local x-centre of each character of `text`, parallel to
    /// `text.chars()`, when the recognizer could derive per-glyph positions
    /// from its CTC output. Empty when unavailable (or after a cache reuse),
    /// in which case hover hit-testing falls back to even character spacing.
    pub char_centers: Vec<f32>,
}

pub trait OcrEngine {
    fn detect(&mut self, frame: &Frame, config: &PipelineConfig) -> Result<Vec<TextBox>>;
    fn recognize(&mut self, frame: &Frame, boxes: &[TextBox]) -> Result<Vec<RecognizedText>>;
}

/// Recognized lines this short (visible, non-whitespace chars) with no
/// alphanumeric content are treated as punctuation/symbol noise.
const MAX_NONALNUM_NOISE_CHARS: usize = 2;
/// A single-character line is only kept if its confidence clears
/// `min_single_character_confidence`; lines longer than this are kept on the
/// ordinary confidence floor.
const MIN_MULTICHAR_LEN: usize = 1;

pub fn filter_recognized_items(
    items: impl IntoIterator<Item = RecognizedText>,
    config: &PipelineConfig,
) -> Vec<RecognizedText> {
    items
        .into_iter()
        .filter(|item| is_usable_recognized_item(item, config))
        .collect()
}

fn is_usable_recognized_item(item: &RecognizedText, config: &PipelineConfig) -> bool {
    let text = item.text.trim();
    if text.is_empty() || item.confidence < config.min_recognition_confidence {
        return false;
    }

    let visible_chars = text.chars().filter(|ch| !ch.is_whitespace()).count();
    if visible_chars <= MAX_NONALNUM_NOISE_CHARS && !text.chars().any(char::is_alphanumeric) {
        return false;
    }

    visible_chars > MIN_MULTICHAR_LEN || item.confidence >= config.min_single_character_confidence
}

pub fn build_ocr_engine(
    config: &RuntimeConfig,
    models: &ModelConfig,
) -> Result<Box<dyn OcrEngine + Send>> {
    match config.backend {
        RuntimeBackend::Mock => Ok(Box::new(MockOcrEngine::default())),
        RuntimeBackend::NvidiaTensorRtThenCuda
        | RuntimeBackend::Cuda
        | RuntimeBackend::DirectMl
        | RuntimeBackend::OpenVino => Ok(Box::new(crate::ort_engine::OrtOcrEngine::new(
            models, config,
        )?)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_empty_and_low_confidence_noise() {
        let config = PipelineConfig::default();
        let items = vec![
            recognized(0, "", 0.99),
            recognized(1, "G", 0.20),
            recognized(2, "V", 0.58),
            recognized(3, "··", 0.95),
            recognized(4, "住", 0.95),
            recognized(5, "A", 0.95),
            recognized(6, "Jess", 0.99),
        ];

        let kept = filter_recognized_items(items, &config)
            .into_iter()
            .map(|item| item.text)
            .collect::<Vec<_>>();

        assert_eq!(kept, vec!["住", "A", "Jess"]);
    }

    fn recognized(id: u64, text: &str, confidence: f32) -> RecognizedText {
        RecognizedText {
            text_box: TextBox {
                id,
                rect: Rect::new(0.0, 0.0, 10.0, 10.0),
                confidence,
                content_fingerprint: id,
            },
            text: text.to_string(),
            confidence,
            reused: false,
            char_centers: Vec::new(),
        }
    }
}

/// Assigns stable ids to detected boxes across frames: any box whose rect
/// quantizes to the same bucket (within `bucket_px`) keeps the same id, so a box
/// that stays roughly put isn't treated as new (and re-recognized) every frame.
/// The quantized tuple key is an implementation detail kept inside this type.
#[derive(Debug, Default)]
pub struct StableIdAllocator {
    ids: HashMap<(i32, i32, i32, i32), u64>,
    next_id: u64,
}

impl StableIdAllocator {
    pub fn id_for(&mut self, rect: Rect, bucket_px: f32) -> u64 {
        let key = rect.quantized_key(bucket_px);
        *self.ids.entry(key).or_insert_with(|| {
            let id = self.next_id;
            self.next_id += 1;
            id
        })
    }
}

#[derive(Debug, Default)]
pub struct MockOcrEngine {
    stable_ids: StableIdAllocator,
}

impl OcrEngine for MockOcrEngine {
    fn detect(&mut self, frame: &Frame, config: &PipelineConfig) -> Result<Vec<TextBox>> {
        let proxy = frame
            .size
            .scaled_to_long_side(config.detector_target_long_side(frame.size.long_side()));
        let sx = frame.size.width as f32 / proxy.width.max(1) as f32;
        let sy = frame.size.height as f32 / proxy.height.max(1) as f32;

        let mut rects = vec![
            Rect::new(64.0, 72.0, 360.0, 48.0).scale(sx, sy),
            Rect::new(64.0, 144.0, 540.0, 52.0).scale(sx, sy),
        ];

        if frame.content_epoch % 2 == 1 {
            rects.push(Rect::new(760.0, 760.0, 280.0, 44.0).scale(sx, sy));
        }

        sort_reading_order(&mut rects);

        let boxes = rects
            .into_iter()
            .take(config.max_boxes_per_frame)
            .map(|rect| TextBox {
                id: self.stable_ids.id_for(rect, 4.0),
                rect: rect.clamp_to(frame.size),
                confidence: 0.92,
                content_fingerprint: frame.content_epoch,
            })
            .collect();

        Ok(boxes)
    }

    fn recognize(&mut self, frame: &Frame, boxes: &[TextBox]) -> Result<Vec<RecognizedText>> {
        Ok(boxes
            .iter()
            .map(|text_box| RecognizedText {
                text_box: text_box.clone(),
                text: format!("mock text {}:{}", frame.content_epoch, text_box.id),
                confidence: 0.88,
                reused: false,
                char_centers: Vec::new(),
            })
            .collect())
    }
}
