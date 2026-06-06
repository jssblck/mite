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
/// Punctuation and symbols that are meaningful OCR output in Japanese UI text.
const MEANINGFUL_SYMBOLS: &[char] = &[
    '。', '、', '・', '…', 'ー', '「', '」', '『', '』', '【', '】', '（', '）', '(', ')', '—',
    '―', '◇', '◆', '％',
];
/// A single-character line is only kept if its confidence clears
/// `min_single_character_confidence`; lines longer than this are kept on the
/// ordinary confidence floor.
const MIN_MULTICHAR_LEN: usize = 1;
/// Low-contrast detector passes often pick up tiny Latin pseudo-text from
/// background decals and UI chrome. Expected ASCII-only UI labels in the eval
/// corpus are taller than this, while the junk runs are microscopic.
const MAX_ASCII_MICROTEXT_HEIGHT: f32 = 12.0;
const MIN_SINGLE_ASCII_WIDTH: f32 = 12.0;

pub fn filter_recognized_items(
    items: impl IntoIterator<Item = RecognizedText>,
    config: &PipelineConfig,
) -> Vec<RecognizedText> {
    let kept = items
        .into_iter()
        .filter(|item| is_usable_recognized_item(item, config))
        .collect::<Vec<_>>();
    merge_adjacent_recognized_fragments(kept)
}

fn is_usable_recognized_item(item: &RecognizedText, config: &PipelineConfig) -> bool {
    let text = item.text.trim();
    if text.is_empty() || item.confidence < config.min_recognition_confidence {
        return false;
    }
    if is_id_like_numeric_noise(text) {
        return false;
    }
    if is_ascii_microtext_noise(item, text) {
        return false;
    }

    let visible_chars = text.chars().filter(|ch| !ch.is_whitespace()).count();
    if visible_chars <= MAX_NONALNUM_NOISE_CHARS
        && !text.chars().any(char::is_alphanumeric)
        && !is_meaningful_symbol_text(text)
    {
        return false;
    }

    visible_chars > MIN_MULTICHAR_LEN || item.confidence >= config.min_single_character_confidence
}

fn is_meaningful_symbol_text(text: &str) -> bool {
    let mut chars = text.chars().filter(|ch| !ch.is_whitespace()).peekable();
    chars.peek().is_some() && chars.all(|ch| MEANINGFUL_SYMBOLS.contains(&ch))
}

fn is_id_like_numeric_noise(text: &str) -> bool {
    let Some((left, right)) = text.trim().split_once('-') else {
        return false;
    };
    !right.contains('-')
        && left.chars().count() >= 3
        && right.chars().count() >= 3
        && left.chars().all(|ch| ch.is_ascii_digit())
        && right.chars().all(|ch| ch.is_ascii_digit())
}

fn is_ascii_microtext_noise(item: &RecognizedText, text: &str) -> bool {
    if !text.chars().all(|ch| ch.is_ascii() && !ch.is_control()) {
        return false;
    }
    let visible_chars = text.chars().filter(|ch| !ch.is_whitespace()).count();
    visible_chars > 0
        && (item.text_box.rect.height <= MAX_ASCII_MICROTEXT_HEIGHT
            || (visible_chars == 1 && item.text_box.rect.width < MIN_SINGLE_ASCII_WIDTH))
}

fn merge_adjacent_recognized_fragments(mut items: Vec<RecognizedText>) -> Vec<RecognizedText> {
    if items.len() < 2 {
        return items;
    }

    items.sort_by(|a, b| {
        a.text_box
            .rect
            .y
            .total_cmp(&b.text_box.rect.y)
            .then_with(|| a.text_box.rect.x.total_cmp(&b.text_box.rect.x))
            .then_with(|| a.text_box.rect.width.total_cmp(&b.text_box.rect.width))
            .then_with(|| a.text_box.rect.height.total_cmp(&b.text_box.rect.height))
            .then_with(|| a.text_box.id.cmp(&b.text_box.id))
    });

    let mut merged = Vec::with_capacity(items.len());
    let mut iter = items.into_iter().peekable();
    while let Some(mut current) = iter.next() {
        while iter
            .peek()
            .is_some_and(|next| should_merge_adjacent_fragments(&current, next))
        {
            let next = iter
                .next()
                .expect("peek confirmed an adjacent recognized fragment");
            current = merge_recognized_pair(current, next);
        }
        merged.push(current);
    }
    merged
}

fn should_merge_adjacent_fragments(left: &RecognizedText, right: &RecognizedText) -> bool {
    let left_text = left.text.trim();
    let right_text = right.text.trim();
    if left_text.len() != left.text.len()
        || right_text.len() != right.text.len()
        || left_text.is_empty()
        || right_text.is_empty()
    {
        return false;
    }

    let a = left.text_box.rect;
    let b = right.text_box.rect;
    if b.x < a.x || b.y < a.y - a.height.max(1.0) * 0.30 {
        return false;
    }

    let avg_height = ((a.height + b.height) / 2.0).max(1.0);
    let height_ratio = a.height.max(b.height) / a.height.min(b.height).max(1.0);
    if height_ratio > 1.35 {
        return false;
    }

    let gap = b.x - a.right();
    if !(-avg_height * 0.15..=avg_height * 0.12).contains(&gap) {
        return false;
    }

    if vertical_overlap_ratio(a, b) < 0.70 {
        return false;
    }

    let Some(left_last) = left_text.chars().next_back() else {
        return false;
    };
    let Some(right_first) = right_text.chars().next() else {
        return false;
    };
    if left_text.chars().count() < 5 || has_recent_fragment_boundary_stop(left_text) {
        return false;
    }
    if !left_text.contains('・') {
        return false;
    }
    is_mergeable_inline_char(left_last)
        && is_mergeable_inline_char(right_first)
        && !is_fragment_boundary_stop(left_last)
}

fn merge_recognized_pair(left: RecognizedText, right: RecognizedText) -> RecognizedText {
    let left_rect = left.text_box.rect;
    let right_rect = right.text_box.rect;
    let x = left_rect.x.min(right_rect.x);
    let y = left_rect.y.min(right_rect.y);
    let right_edge = left_rect.right().max(right_rect.right());
    let bottom = left_rect.bottom().max(right_rect.bottom());
    let left_chars = left.text.chars().count();
    let right_chars = right.text.chars().count();

    let char_centers =
        if left.char_centers.len() == left_chars && right.char_centers.len() == right_chars {
            left.char_centers
                .iter()
                .chain(right.char_centers.iter())
                .copied()
                .collect()
        } else {
            Vec::new()
        };

    let raw_text = [left.text.as_str(), right.text.as_str()].concat();
    let text = crate::text_corrections::apply_common_replacements(&raw_text);
    let char_centers = if text.chars().count() == raw_text.chars().count() {
        char_centers
    } else {
        Vec::new()
    };
    let text_box = TextBox {
        id: left.text_box.id,
        rect: Rect::new(x, y, right_edge - x, bottom - y),
        confidence: left.text_box.confidence.min(right.text_box.confidence),
        content_fingerprint: combine_fragment_fingerprints(
            left.text_box.content_fingerprint,
            right.text_box.content_fingerprint,
        ),
    };
    let confidence =
        weighted_confidence(left.confidence, left_chars, right.confidence, right_chars);
    let reused = left.reused && right.reused;

    RecognizedText {
        text_box,
        text,
        confidence,
        reused,
        char_centers,
    }
}

fn vertical_overlap_ratio(a: Rect, b: Rect) -> f32 {
    let overlap = (a.bottom().min(b.bottom()) - a.y.max(b.y)).max(0.0);
    let narrow = a.height.min(b.height).max(1.0);
    overlap / narrow
}

fn is_mergeable_inline_char(ch: char) -> bool {
    ch.is_alphanumeric()
        || matches!(
            ch,
            '\u{3040}'..='\u{309f}'
                | '\u{30a0}'..='\u{30ff}'
                | '\u{3400}'..='\u{9fff}'
                | '\u{ff10}'..='\u{ff19}'
                | '\u{ff21}'..='\u{ff3a}'
                | '\u{ff41}'..='\u{ff5a}'
                | '％'
                | '%'
                | '+'
                | '-'
                | '/'
                | '.'
                | ':'
        )
}

fn is_fragment_boundary_stop(ch: char) -> bool {
    matches!(
        ch,
        '。' | '．'
            | '.'
            | '！'
            | '!'
            | '？'
            | '?'
            | '、'
            | ','
            | '」'
            | '』'
            | '）'
            | ')'
            | ']'
            | '］'
    )
}

fn has_recent_fragment_boundary_stop(text: &str) -> bool {
    text.chars().rev().take(3).any(is_fragment_boundary_stop)
}

fn weighted_confidence(left: f32, left_chars: usize, right: f32, right_chars: usize) -> f32 {
    let total_chars = left_chars + right_chars;
    if total_chars == 0 {
        return left.min(right);
    }
    ((left * left_chars as f32) + (right * right_chars as f32)) / total_chars as f32
}

fn combine_fragment_fingerprints(left: u64, right: u64) -> u64 {
    left.rotate_left(17) ^ right.rotate_right(7) ^ 0x9e37_79b9_7f4a_7c15
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
            recognized(7, "。", 0.95),
            recognized(8, "◇", 0.95),
            recognized(9, "……", 0.95),
            recognized(10, "・", 0.95),
        ];

        let kept = filter_recognized_items(items, &config)
            .into_iter()
            .map(|item| item.text)
            .collect::<Vec<_>>();

        assert_eq!(kept, vec!["住", "A", "Jess", "。", "◇", "……", "・"]);
    }

    #[test]
    fn filters_id_like_numeric_noise_without_dropping_ui_numbers() {
        let config = PipelineConfig::default();
        let items = vec![
            recognized(0, "001-001", 0.99),
            recognized(1, "101-001", 0.99),
            recognized(2, "2026/05/22", 0.99),
            recognized(3, "0/6", 0.99),
            recognized(4, "3840x2160", 0.99),
            recognized(5, "65", 0.99),
        ];

        let kept = filter_recognized_items(items, &config)
            .into_iter()
            .map(|item| item.text)
            .collect::<Vec<_>>();

        assert_eq!(kept, vec!["2026/05/22", "0/6", "3840x2160", "65"]);
    }

    #[test]
    fn filters_tiny_ascii_microtext_without_dropping_ui_labels() {
        let config = PipelineConfig::default();
        let items = vec![
            recognized_at(0, Rect::new(2607.0, 409.0, 107.0, 9.0), "\"OO 2 DTM", 0.99),
            recognized_at(1, Rect::new(863.0, 328.0, 8.0, 23.0), "A", 0.99),
            recognized_at(2, Rect::new(3087.0, 558.0, 52.0, 24.0), "HP", 0.99),
            recognized_at(3, Rect::new(3038.0, 458.0, 134.0, 29.0), "Lv.1", 0.99),
            recognized_at(4, Rect::new(3246.0, 1198.0, 16.0, 15.0), "S", 0.99),
            recognized_at(5, Rect::new(2159.0, 1674.0, 39.0, 25.0), "x5", 0.99),
        ];

        let kept = filter_recognized_items(items, &config)
            .into_iter()
            .map(|item| item.text)
            .collect::<Vec<_>>();

        assert_eq!(kept, vec!["Lv.1", "HP", "S", "x5"]);
    }

    #[test]
    fn merges_tightly_adjacent_same_line_fragments() {
        let config = PipelineConfig::default();
        let items = vec![
            recognized_at(
                0,
                Rect::new(251.0, 451.0, 380.0, 21.0),
                "ラハイロイ・エンドボ",
                0.99,
            ),
            recognized_at(1, Rect::new(632.0, 452.0, 147.0, 21.0), "イスエビ", 0.93),
        ];

        let kept = filter_recognized_items(items, &config);

        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].text, "ラハイロイ・エンドボイスエピ");
        assert_eq!(kept[0].text_box.rect, Rect::new(251.0, 451.0, 528.0, 22.0));
    }

    #[test]
    fn keeps_separate_same_row_labels_when_gap_is_not_tiny() {
        let config = PipelineConfig::default();
        let items = vec![
            recognized_at(0, Rect::new(100.0, 200.0, 60.0, 24.0), "外す", 0.99),
            recognized_at(1, Rect::new(190.0, 200.0, 70.0, 24.0), "強化", 0.99),
        ];

        let kept = filter_recognized_items(items, &config);

        assert_eq!(
            kept.into_iter().map(|item| item.text).collect::<Vec<_>>(),
            vec!["外す", "強化"]
        );
    }

    #[test]
    fn keeps_fragments_separate_after_sentence_boundary() {
        let config = PipelineConfig::default();
        let items = vec![
            recognized_at(0, Rect::new(100.0, 200.0, 110.0, 24.0), "完了した。1", 0.99),
            recognized_at(1, Rect::new(211.0, 200.0, 72.0, 24.0), "次項目", 0.99),
        ];

        let kept = filter_recognized_items(items, &config);

        assert_eq!(
            kept.into_iter().map(|item| item.text).collect::<Vec<_>>(),
            vec!["完了した。1", "次項目"]
        );
    }

    #[test]
    fn keeps_short_prefix_separate_from_adjacent_title() {
        let config = PipelineConfig::default();
        let items = vec![
            recognized_at(0, Rect::new(995.0, 1259.0, 148.0, 31.0), "今期の", 0.99),
            recognized_at(
                1,
                Rect::new(1144.0, 1261.0, 399.0, 28.0),
                "千の扉の奇想",
                0.99,
            ),
        ];

        let kept = filter_recognized_items(items, &config);

        assert_eq!(
            kept.into_iter().map(|item| item.text).collect::<Vec<_>>(),
            vec!["今期の", "千の扉の奇想"]
        );
    }

    #[test]
    fn keeps_prose_fragments_separate_without_compound_name_marker() {
        let config = PipelineConfig::default();
        let items = vec![
            recognized_at(
                0,
                Rect::new(120.0, 515.0, 460.0, 23.0),
                "周囲の目標を牽引して集",
                0.99,
            ),
            recognized_at(
                1,
                Rect::new(581.0, 515.0, 462.0, 25.0),
                "焦熱ダメージを与える。",
                0.99,
            ),
        ];

        let kept = filter_recognized_items(items, &config);

        assert_eq!(
            kept.into_iter().map(|item| item.text).collect::<Vec<_>>(),
            vec!["周囲の目標を牽引して集", "焦熱ダメージを与える。"]
        );
    }

    fn recognized(id: u64, text: &str, confidence: f32) -> RecognizedText {
        recognized_at(id, Rect::new(0.0, 0.0, 80.0, 24.0), text, confidence)
    }

    fn recognized_at(id: u64, rect: Rect, text: &str, confidence: f32) -> RecognizedText {
        RecognizedText {
            text_box: TextBox {
                id,
                rect,
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
