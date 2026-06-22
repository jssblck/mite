//! Pure, testable geometry/text logic for the interactive hover overlay.
//!
//! The Win32 loop (see [`crate::interactive`]) feeds raw cursor coordinates and
//! recognized boxes through these helpers to decide which word is under the
//! cursor and what the definition popup should say. Keeping this logic free of
//! any Win32 types lets it be unit-tested off-Windows.

use serde::{Deserialize, Serialize};

use crate::dictionary::{RubySegment, Token};
use crate::geometry::{Rect, ScreenRect};
use crate::pos::{LinderaPos, PosClass};
use crate::script::{is_cjk, is_kana};
use crate::text_blocks::LineToken;

mod furigana;
mod pill;
mod sense;

pub use furigana::{FuriSegment, furigana_segments, surface_furigana};
pub use pill::pill_label;
pub use sense::{SenseHint, transitivity_hint};

use furigana::has_kanji;
use sense::ordered_sense_indices;

/// Upper bound (exclusive of 1.0) for the within-box character fraction, so the
/// last character maps to index `char_count - 1` rather than off the end.
const CHAR_INDEX_FRACTION_CAP: f32 = 0.999_999;

/// Highlight padding around a detected word box (detection boxes hug the
/// glyphs). Vertical padding applies to every word; horizontal padding extends
/// only a line's outer ends so adjacent words never overlap. Each is a fraction
/// of the box height, clamped to a pixel range.
const VPAD_RATIO: f32 = 0.12;
const VPAD_MIN_PX: f32 = 2.0;
const VPAD_MAX_PX: f32 = 6.0;
const HPAD_RATIO: f32 = 0.15;
const HPAD_MIN_PX: f32 = 2.0;
const HPAD_MAX_PX: f32 = 8.0;

/// Convert a screen-space cursor position to coordinates local to a captured
/// frame (whose origin on screen is `frame.x`/`frame.y`).
pub fn cursor_to_local(cursor_x: i32, cursor_y: i32, frame: ScreenRect) -> (f32, f32) {
    ((cursor_x - frame.x) as f32, (cursor_y - frame.y) as f32)
}

/// Index of the recognized box under a frame-local point. When several boxes
/// overlap the point we pick the smallest-area one, which is the most specific
/// (e.g. an inner label over an enclosing panel).
pub fn hit_test(rects: &[Rect], x: f32, y: f32) -> Option<usize> {
    rects
        .iter()
        .enumerate()
        .filter(|(_, r)| r.contains(x, y))
        .min_by(|(_, a), (_, b)| a.area().total_cmp(&b.area()))
        .map(|(i, _)| i)
}

/// Estimate the character index under a local x position within a box that
/// renders `char_count` roughly evenly-spaced characters. Result is clamped to
/// `[0, char_count - 1]`. Returns 0 for an empty box.
pub fn char_index_at(rect: Rect, char_count: usize, local_x: f32) -> usize {
    if char_count == 0 {
        return 0;
    }
    let width = rect.width.max(1.0);
    let frac = ((local_x - rect.x) / width).clamp(0.0, CHAR_INDEX_FRACTION_CAP);
    ((frac * char_count as f32) as usize).min(char_count - 1)
}

/// Locate each token's `[start, end)` span in character coordinates of `line`.
///
/// [`crate::dictionary::Dictionary::analyze_line`] skips whitespace between
/// tokens, so cumulative surface lengths can drift from raw character offsets.
/// We recover exact offsets by scanning `line` for each surface in order.
pub fn token_spans(line: &str, tokens: &[Token]) -> Vec<(usize, usize)> {
    let chars: Vec<char> = line.chars().collect();
    let mut spans = Vec::with_capacity(tokens.len());
    let mut pos = 0usize;
    for token in tokens {
        let surface: Vec<char> = token.surface.chars().collect();
        while pos < chars.len() && !matches_at(&chars, pos, &surface) {
            pos += 1;
        }
        let start = pos.min(chars.len());
        let end = (start + surface.len()).min(chars.len());
        spans.push((start, end));
        pos = end.max(start + 1).min(chars.len());
    }
    spans
}

fn matches_at(haystack: &[char], at: usize, needle: &[char]) -> bool {
    if needle.is_empty() || at + needle.len() > haystack.len() {
        return false;
    }
    haystack[at..at + needle.len()] == *needle
}

/// Index of the token whose span covers `char_index`, given precomputed spans.
pub fn token_at_char(spans: &[(usize, usize)], char_index: usize) -> Option<usize> {
    spans
        .iter()
        .position(|&(start, end)| char_index >= start && char_index < end)
}

/// Index of the character whose centre is nearest a local x position, given
/// per-character centre coordinates (frame-local, parallel to the line's
/// chars). Returns `None` when no per-glyph centres are available.
pub fn char_index_from_centers(centers: &[f32], local_x: f32) -> Option<usize> {
    let mut best = None;
    let mut best_distance = f32::INFINITY;
    for (index, &center) in centers.iter().enumerate() {
        let distance = (center - local_x).abs();
        if distance < best_distance {
            best_distance = distance;
            best = Some(index);
        }
    }
    best
}

/// Resolve the token under a cursor over a recognized box in one step: map the
/// local x to a character, then to the covering token. `line` is the box's
/// recognized text and `tokens` its segmentation. When per-glyph `centers` are
/// available (from the recognizer's CTC positions) they give an exact
/// character; otherwise we fall back to even spacing across the box.
pub fn token_under_cursor(
    rect: Rect,
    line: &str,
    tokens: &[Token],
    centers: &[f32],
    local_x: f32,
) -> Option<usize> {
    let char_index = char_index_from_centers(centers, local_x)
        .unwrap_or_else(|| char_index_at(rect, line.chars().count(), local_x));
    let spans = token_spans(line, tokens);
    token_at_char(&spans, char_index)
}

/// Grammatical category of a word, used to colour its highlight overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum WordCategory {
    Particle,
    Noun,
    Verb,
    Adjective,
    Adverb,
    Expression,
    Auxiliary,
    #[default]
    Other,
    Unknown,
}

/// Classify a token by part of speech (from its first JMdict entry). Unknown
/// tokens (no entry) get [`WordCategory::Unknown`].
pub fn categorize(token: &Token) -> WordCategory {
    if !token.is_known() {
        return token
            .source_pos
            .map(category_from_lindera_pos)
            .unwrap_or(WordCategory::Unknown);
    }
    token
        .entries
        .first()
        .and_then(|entry| entry.senses.first())
        .and_then(|sense| sense.part_of_speech.iter().find(|pos| !pos.is_empty()))
        .map(|pos| category_from_pos(pos))
        .unwrap_or(WordCategory::Other)
}

fn category_from_lindera_pos(pos: LinderaPos) -> WordCategory {
    match pos {
        LinderaPos::Particle => WordCategory::Particle,
        LinderaPos::AuxVerb => WordCategory::Auxiliary,
        LinderaPos::Verb => WordCategory::Verb,
        LinderaPos::Adjective => WordCategory::Adjective,
        LinderaPos::Adverb => WordCategory::Adverb,
        LinderaPos::Adnominal => WordCategory::Adjective,
        LinderaPos::Conjunction
        | LinderaPos::Interjection
        | LinderaPos::Prefix
        | LinderaPos::Other => WordCategory::Other,
        LinderaPos::Noun => WordCategory::Noun,
    }
}

fn category_from_pos(pos: &str) -> WordCategory {
    match PosClass::of(pos) {
        PosClass::Particle => WordCategory::Particle,
        PosClass::Noun => WordCategory::Noun,
        PosClass::Verb => WordCategory::Verb,
        PosClass::Adjective => WordCategory::Adjective,
        PosClass::Adverb => WordCategory::Adverb,
        PosClass::Expression => WordCategory::Expression,
        PosClass::Auxiliary => WordCategory::Auxiliary,
        PosClass::Other => WordCategory::Other,
    }
}

/// The portion of a gloss line to display, with the trailing `  (pos)` tag
/// removed. The structured gloss keeps the tag so the eval guard can score
/// grammatical class, but the overlay shows the class in the category pill
/// instead, so the visible gloss is just the meanings. Only a tag introduced by
/// the two-space [`crate::dictionary`] convention is stripped; parentheticals
/// that are part of a meaning (single-spaced, mid-gloss) are left intact.
pub fn strip_pos_tag(gloss: &str) -> &str {
    match gloss.rsplit_once("  (") {
        Some((head, tail)) if tail.ends_with(')') && !tail[..tail.len() - 1].contains(')') => head,
        _ => gloss,
    }
}

/// Length, in UTF-16 code units, of an inflection note's lead word when it is a
/// plain-English term worth emphasising (e.g. the "Polite" in "Polite masu-form:
/// ..."). Returns 0 for notes that open with Japanese grammar terms, which the
/// popup then renders without a bold lead. ASCII lead words are all in the BMP,
/// so the code-unit count equals the character count and can index the wrapped
/// UTF-16 line directly.
pub fn note_lead_len(note: &str) -> usize {
    if !note.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) {
        return 0;
    }
    note.chars()
        .take_while(|c| c.is_ascii_alphabetic())
        .map(char::len_utf16)
        .sum()
}

/// Structured content for the definition popup of one word.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct PopupContent {
    /// Dictionary (headword) form, shown large.
    pub word: String,
    /// Headword split into ruby segments (kanji runs carry their furigana).
    pub ruby: Vec<FuriSegment>,
    /// A note about the surface/inflection (e.g. `食べ · 連用形`), if inflected.
    pub note: Option<String>,
    /// Per-sense gloss lines (the meanings, joined with `; `), already formatted.
    pub glosses: Vec<String>,
    pub category: WordCategory,
    /// Plain-language grammar tag for the category pill (`NOUN`,
    /// `VERB (GODAN)`, ...), or `None` when there is nothing useful to name.
    pub pill: Option<String>,
}

/// Build the popup content for a resolved token: word, furigana ruby, an
/// inflection note, and the first entry's senses, ordered by [`SenseHint`].
pub fn popup_content(
    token: &Token,
    hint: SenseHint,
    max_senses: usize,
    max_glosses: usize,
) -> PopupContent {
    let word = token.dictionary_form.clone();
    let entry = token.entries.first();
    let ruby = if is_quiet_symbol_surface(&token.surface) && token.surface == token.dictionary_form
    {
        Vec::new()
    } else {
        entry
            .and_then(|entry| entry.popup_override.as_ref())
            .map(|popup| ruby_segments_from_override(&popup.ruby))
            .unwrap_or_else(|| {
                let reading = entry
                    .and_then(|entry| entry.kana.first())
                    .filter(|reading| has_kanji(&word) && reading.as_str() != word)
                    .cloned();
                if entry.is_none() && is_quiet_unknown_surface(&token.surface) {
                    Vec::new()
                } else if entry.is_none()
                    && token.source_pos == Some(crate::pos::LinderaPos::AuxVerb)
                    && token.surface != token.dictionary_form
                {
                    vec![FuriSegment {
                        text: token.surface.clone(),
                        furigana: None,
                    }]
                } else {
                    match &reading {
                        Some(reading) => furigana_segments(&word, reading),
                        None => vec![FuriSegment {
                            text: word.clone(),
                            furigana: None,
                        }],
                    }
                }
            })
    };

    let note = if let Some(note) = &token.note_override {
        Some(note.clone())
    } else if token.surface != token.dictionary_form {
        let reasons = if token.reasons.is_empty() {
            String::new()
        } else {
            format!(" · {}", token.reasons.join(" < "))
        };
        Some(format!("{}{reasons}", token.surface))
    } else if !token.reasons.is_empty() {
        Some(token.reasons.join(" < "))
    } else {
        None
    };

    let mut glosses = Vec::new();
    if let Some(popup) = entry.and_then(|entry| entry.popup_override.as_ref()) {
        glosses = popup.glosses.clone();
    } else if let Some(entry) = entry {
        let order = ordered_sense_indices(&entry.senses, hint);
        for &index in order.iter().take(max_senses) {
            let sense = &entry.senses[index];
            let text = sense
                .glosses
                .iter()
                .take(max_glosses)
                .cloned()
                .collect::<Vec<_>>()
                .join("; ");
            let pos = if sense.part_of_speech.is_empty() {
                String::new()
            } else {
                format!("  ({})", sense.part_of_speech.join(", "))
            };
            glosses.push(format!("{text}{pos}"));
        }
    }

    PopupContent {
        word,
        ruby,
        note,
        glosses,
        category: categorize(token),
        pill: pill_label(token),
    }
}

fn is_quiet_unknown_surface(surface: &str) -> bool {
    !surface.is_empty()
        && surface.chars().all(|ch| {
            is_quiet_symbol_char(ch)
                || ch.is_ascii_alphanumeric()
                || !is_cjk(ch) && !is_kana(ch) && !ch.is_ascii_alphabetic()
        })
}

fn is_quiet_symbol_surface(surface: &str) -> bool {
    !surface.is_empty() && surface.chars().all(is_quiet_symbol_char)
}

fn is_quiet_symbol_char(ch: char) -> bool {
    matches!(
        ch,
        '、' | '。' | '・' | '：' | ':' | ',' | '.' | '!' | '?' | '！' | '？'
    )
}

/// A drawable highlight: a word's frame-local rect, its category (for colour),
/// whether it resolved to a dictionary entry, and the ruby to draw over the
/// on-screen surface glyphs.
#[derive(Debug, Clone, PartialEq)]
pub struct Highlight {
    pub rect: Rect,
    pub category: WordCategory,
    pub known: bool,
    /// Surface-aligned ruby: each segment spans consecutive surface characters,
    /// kanji runs carrying the furigana to draw above them.
    pub ruby: Vec<FuriSegment>,
}

/// A segmented word: its highlight geometry plus the popup content to show when
/// the cursor is over it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WordSpan {
    pub rect: Rect,
    /// Surface form as recognized (before lemmatization).
    pub surface: String,
    pub category: WordCategory,
    pub known: bool,
    pub content: PopupContent,
    /// Ruby aligned to the surface glyphs, for the always-on overlay furigana
    /// (the popup ruby in `content` is keyed to the dictionary form instead).
    #[serde(skip)]
    pub surface_ruby: Vec<FuriSegment>,
}

impl WordSpan {
    pub fn highlight(&self) -> Highlight {
        Highlight {
            rect: self.rect,
            category: self.category,
            known: self.known,
            ruby: self.surface_ruby.clone(),
        }
    }
}

/// Frame-local pixel rect of the word covering chars `[start, end)` within a
/// box. Uses per-glyph `centers` (boundaries at midpoints between adjacent char
/// centres) when available, else falls back to even spacing across the box.
pub fn word_rect(
    box_rect: Rect,
    centers: &[f32],
    char_count: usize,
    start: usize,
    end: usize,
) -> Rect {
    let count = char_count.max(1);
    let (left, right) = if centers.len() == char_count && start < end {
        let left = if start == 0 {
            box_rect.x
        } else {
            (centers[start - 1] + centers[start]) / 2.0
        };
        let right = if end >= centers.len() {
            box_rect.right()
        } else {
            (centers[end - 1] + centers[end]) / 2.0
        };
        (left, right)
    } else {
        let width = box_rect.width / count as f32;
        (
            box_rect.x + start as f32 * width,
            box_rect.x + end as f32 * width,
        )
    };
    let left = left.max(box_rect.x);
    let right = right.min(box_rect.right()).max(left + 1.0);
    Rect::new(left, box_rect.y, right - left, box_rect.height)
}

/// Segment one recognized box into drawable/hover-able word spans.
pub fn build_word_spans(
    box_rect: Rect,
    line: &str,
    tokens: &[Token],
    centers: &[f32],
    max_senses: usize,
    max_glosses: usize,
) -> Vec<WordSpan> {
    let char_count = line.chars().count();
    let spans = token_spans(line, tokens);
    let mut words: Vec<WordSpan> = tokens
        .iter()
        .zip(spans)
        .enumerate()
        .map(|(index, (token, (start, end)))| {
            let hint = transitivity_hint(tokens, index);
            let content = popup_content(token, hint, max_senses, max_glosses);
            let surface_ruby = surface_furigana(&token.surface, &content.ruby);
            WordSpan {
                rect: word_rect(box_rect, centers, char_count, start, end),
                surface: token.surface.clone(),
                category: categorize(token),
                known: token.is_known(),
                content,
                surface_ruby,
            }
        })
        .collect();

    pad_word_spans(box_rect, &mut words);
    words
}

/// Segment one recognized line into drawable/hover-able word spans when lookup
/// was performed over a multi-line text block. A returned span may represent
/// only the visible part of a token that wraps onto another line.
pub fn build_word_spans_from_line_tokens(
    box_rect: Rect,
    line: &str,
    tokens: &[LineToken],
    block_tokens: &[Token],
    centers: &[f32],
    max_senses: usize,
    max_glosses: usize,
) -> Vec<WordSpan> {
    let char_count = line.chars().count();
    let mut words: Vec<WordSpan> = tokens
        .iter()
        .map(|segment| {
            let hint = transitivity_hint(block_tokens, segment.block_token_index);
            let content = popup_content(&segment.token, hint, max_senses, max_glosses);
            // Ruby is aligned to the *visible* surface so wrapped tokens only
            // carry furigana for the kanji actually shown on this line.
            let surface_ruby = surface_furigana(&segment.visible_surface, &content.ruby);
            WordSpan {
                rect: word_rect(
                    box_rect,
                    centers,
                    char_count,
                    segment.span.start,
                    segment.span.end,
                ),
                surface: segment.visible_surface.clone(),
                category: categorize(&segment.token),
                known: segment.token.is_known(),
                content,
                surface_ruby,
            }
        })
        .collect();

    pad_word_spans(box_rect, &mut words);
    words
}

fn pad_word_spans(box_rect: Rect, words: &mut [WordSpan]) {
    // Pad highlights outward for clarity (detection boxes hug the glyphs).
    // Vertical padding applies to every word; horizontal padding only extends
    // the line's outer ends, so adjacent words within a line never overlap.
    let vpad = (box_rect.height * VPAD_RATIO).clamp(VPAD_MIN_PX, VPAD_MAX_PX);
    let hpad = (box_rect.height * HPAD_RATIO).clamp(HPAD_MIN_PX, HPAD_MAX_PX);
    let last = words.len().saturating_sub(1);
    for (index, word) in words.iter_mut().enumerate() {
        word.rect.y -= vpad;
        word.rect.height += 2.0 * vpad;
        if index == 0 {
            word.rect.x -= hpad;
            word.rect.width += hpad;
        }
        if index == last {
            word.rect.width += hpad;
        }
    }
}

fn ruby_segments_from_override(segments: &[RubySegment]) -> Vec<FuriSegment> {
    segments
        .iter()
        .map(|segment| FuriSegment {
            text: segment.text.clone(),
            furigana: segment.furigana.clone(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dictionary::{Entry, Sense};

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect::new(x, y, w, h)
    }

    #[test]
    fn maps_cursor_into_frame_local_space() {
        let frame = ScreenRect::new(100, 50, crate::geometry::Size::new(800, 600));
        assert_eq!(cursor_to_local(140, 80, frame), (40.0, 30.0));
    }

    #[test]
    fn hit_test_prefers_smallest_containing_box() {
        let rects = [rect(0.0, 0.0, 100.0, 100.0), rect(10.0, 10.0, 20.0, 20.0)];
        assert_eq!(hit_test(&rects, 15.0, 15.0), Some(1));
        assert_eq!(hit_test(&rects, 5.0, 5.0), Some(0));
        assert_eq!(hit_test(&rects, 200.0, 200.0), None);
    }

    #[test]
    fn char_index_splits_box_width_evenly() {
        let r = rect(0.0, 0.0, 100.0, 20.0);
        assert_eq!(char_index_at(r, 10, 5.0), 0);
        assert_eq!(char_index_at(r, 10, 55.0), 5);
        assert_eq!(char_index_at(r, 10, 99.0), 9);
        // Out-of-range clamps to the ends.
        assert_eq!(char_index_at(r, 10, -20.0), 0);
        assert_eq!(char_index_at(r, 10, 500.0), 9);
    }

    #[test]
    fn token_spans_recover_offsets_across_whitespace() {
        // Surfaces "今日" and "晴れ" with a space the tokenizer drops.
        let tokens = vec![tok("今日"), tok("晴れ")];
        let spans = token_spans("今日 晴れ", &tokens);
        assert_eq!(spans, vec![(0, 2), (3, 5)]);
        // Char index 3 (the 晴) maps to the second token; the space (index 2) to none.
        assert_eq!(token_at_char(&spans, 0), Some(0));
        assert_eq!(token_at_char(&spans, 2), None);
        assert_eq!(token_at_char(&spans, 3), Some(1));
    }

    #[test]
    fn token_under_cursor_resolves_word_by_even_spacing() {
        let line = "水を飲む";
        let tokens = vec![tok("水"), tok("を"), tok("飲む")];
        let r = rect(0.0, 0.0, 120.0, 30.0); // 4 chars => 30px each
        // No centres: even-spacing fallback. x in the last cell (90..120) lands
        // on 飲む (chars 2..4); x in the first cell lands on 水.
        assert_eq!(token_under_cursor(r, line, &tokens, &[], 100.0), Some(2));
        assert_eq!(token_under_cursor(r, line, &tokens, &[], 10.0), Some(0));
    }

    #[test]
    fn token_under_cursor_uses_centers_when_present() {
        let line = "水を飲む";
        let tokens = vec![tok("水"), tok("を"), tok("飲む")];
        let r = rect(0.0, 0.0, 120.0, 30.0);
        // Uneven real glyph centres: 水 wide, を narrow, then 飲む.
        let centers = [10.0, 40.0, 70.0, 110.0];
        // Cursor at 65 is nearest the 飲 centre (70) -> token 飲む.
        assert_eq!(
            token_under_cursor(r, line, &tokens, &centers, 65.0),
            Some(2)
        );
        // Cursor at 38 is nearest を (40) -> token を.
        assert_eq!(
            token_under_cursor(r, line, &tokens, &centers, 38.0),
            Some(1)
        );
    }

    #[test]
    fn char_index_from_centers_picks_nearest() {
        let centers = [10.0, 40.0, 70.0];
        assert_eq!(char_index_from_centers(&centers, 8.0), Some(0));
        assert_eq!(char_index_from_centers(&centers, 52.0), Some(1));
        assert_eq!(char_index_from_centers(&centers, 200.0), Some(2));
        assert_eq!(char_index_from_centers(&[], 5.0), None);
    }

    fn verb_token() -> Token {
        let mut token = tok("飲ん");
        token.dictionary_form = "飲む".to_string();
        token.reasons = vec!["連用形".to_string()];
        token.entries = vec![Entry {
            kanji: vec!["飲む".to_string()],
            kana: vec!["のむ".to_string()],
            senses: vec![Sense {
                part_of_speech: vec!["v5m".to_string()],
                glosses: vec!["to drink".to_string(), "to gulp".to_string()],
                misc: Vec::new(),
            }],
            common: true,
            popup_override: None,
        }];
        token
    }

    #[test]
    fn popup_content_carries_word_ruby_note_and_glosses() {
        let content = popup_content(&verb_token(), SenseHint::default(), 3, 4);
        assert_eq!(content.word, "飲む");
        // 飲む / のむ → 飲[の] + む(no furigana).
        assert_eq!(
            content.ruby,
            vec![
                FuriSegment {
                    text: "飲".to_string(),
                    furigana: Some("の".to_string())
                },
                FuriSegment {
                    text: "む".to_string(),
                    furigana: None
                },
            ]
        );
        assert_eq!(content.note.as_deref(), Some("飲ん · 連用形"));
        // The structured gloss keeps its POS tag (the eval guard scores it); the
        // overlay strips the tag at draw time and shows the class in the pill.
        assert_eq!(
            content.glosses,
            vec!["to drink; to gulp  (v5m)".to_string()]
        );
        assert_eq!(content.category, WordCategory::Verb);
        assert_eq!(content.pill.as_deref(), Some("VERB (GODAN)"));
    }

    #[test]
    fn popup_content_has_no_furigana_for_kana_word() {
        let mut token = tok("を");
        token.entries = vec![Entry {
            kanji: vec![],
            kana: vec!["を".to_string()],
            senses: vec![Sense {
                part_of_speech: vec!["prt".to_string()],
                glosses: vec!["[object marker]".to_string()],
                misc: Vec::new(),
            }],
            common: true,
            popup_override: None,
        }];
        let content = popup_content(&token, SenseHint::default(), 3, 4);
        assert_eq!(
            content.ruby,
            vec![FuriSegment {
                text: "を".to_string(),
                furigana: None
            }]
        );
        assert_eq!(content.category, WordCategory::Particle);
    }

    #[test]
    fn popup_content_uses_surface_ruby_for_unknown_auxiliary() {
        let mut token = tok("ました");
        token.dictionary_form = "ます".to_string();
        token.reasons = vec!["丁寧".to_string(), "過去".to_string()];
        token.source_pos = Some(LinderaPos::AuxVerb);
        token.note_override = Some("Polite past auxiliary.".to_string());

        let content = popup_content(&token, SenseHint::default(), 3, 4);

        assert_eq!(content.word, "ます");
        assert_eq!(
            content.ruby,
            vec![FuriSegment {
                text: "ました".to_string(),
                furigana: None,
            }]
        );
        assert_eq!(content.note.as_deref(), Some("Polite past auxiliary."));
        assert_eq!(content.category, WordCategory::Auxiliary);
    }

    #[test]
    fn popup_content_has_no_ruby_for_unknown_punctuation_or_numbers() {
        for surface in ["、", "。", "・", "：", "9"] {
            let content = popup_content(&tok(surface), SenseHint::default(), 3, 4);
            assert_eq!(content.ruby, Vec::<FuriSegment>::new());
            assert_eq!(content.category, WordCategory::Unknown);
        }
    }

    #[test]
    fn popup_content_has_no_ruby_for_known_punctuation() {
        for surface in ["、", "。", "・", "："] {
            let content = popup_content(
                &Token {
                    surface: surface.to_string(),
                    dictionary_form: surface.to_string(),
                    reasons: Vec::new(),
                    entries: vec![Entry {
                        kanji: Vec::new(),
                        kana: Vec::new(),
                        senses: vec![Sense {
                            part_of_speech: vec!["unc".to_string()],
                            glosses: Vec::new(),
                            misc: Vec::new(),
                        }],
                        common: true,
                        popup_override: None,
                    }],
                    source_pos: Some(LinderaPos::Other),
                    note_override: None,
                },
                SenseHint::default(),
                3,
                4,
            );
            assert_eq!(content.ruby, Vec::<FuriSegment>::new());
        }
    }

    #[test]
    fn popup_content_keeps_surface_ruby_for_unknown_text_fragments() {
        let surface = "ダーニャ";
        let content = popup_content(&tok(surface), SenseHint::default(), 3, 4);
        assert_eq!(
            content.ruby,
            vec![FuriSegment {
                text: surface.to_string(),
                furigana: None,
            }]
        );
        assert_eq!(content.category, WordCategory::Unknown);
    }

    #[test]
    fn popup_content_quiets_ascii_abbreviation_unknowns() {
        let mut token = tok("EXP");
        token.source_pos = Some(LinderaPos::Other);

        let content = popup_content(&token, SenseHint::default(), 3, 4);

        assert_eq!(content.ruby, Vec::<FuriSegment>::new());
        assert_eq!(content.category, WordCategory::Other);
    }

    #[test]
    fn furigana_distributes_reading_over_kanji_runs() {
        // Trailing okurigana: 高い / たかい → 高[たか] + い.
        assert_eq!(
            furigana_segments("高い", "たかい"),
            vec![
                FuriSegment {
                    text: "高".to_string(),
                    furigana: Some("たか".to_string())
                },
                FuriSegment {
                    text: "い".to_string(),
                    furigana: None
                },
            ]
        );
        // Interleaved: 取り出す / とりだす → 取[と] り 出[だ] す.
        assert_eq!(
            furigana_segments("取り出す", "とりだす"),
            vec![
                FuriSegment {
                    text: "取".to_string(),
                    furigana: Some("と".to_string())
                },
                FuriSegment {
                    text: "り".to_string(),
                    furigana: None
                },
                FuriSegment {
                    text: "出".to_string(),
                    furigana: Some("だ".to_string())
                },
                FuriSegment {
                    text: "す".to_string(),
                    furigana: None
                },
            ]
        );
        // Solid kanji compound: whole reading over the run.
        assert_eq!(
            furigana_segments("聖遺物", "せいいぶつ"),
            vec![FuriSegment {
                text: "聖遺物".to_string(),
                furigana: Some("せいいぶつ".to_string())
            }]
        );
    }

    #[test]
    fn furigana_falls_back_when_kana_mismatch() {
        // Word kana don't appear in the reading → whole-word ruby.
        assert_eq!(
            furigana_segments("食べる", "たべXX"),
            vec![FuriSegment {
                text: "食べる".to_string(),
                furigana: Some("たべXX".to_string())
            }]
        );
    }

    #[test]
    fn note_lead_len_marks_only_english_lead_words() {
        assert_eq!(note_lead_len("Polite masu-form: the polite non-past."), 6);
        assert_eq!(note_lead_len("Honorific prefix."), 9);
        // A note that opens with Japanese grammar terms has no bold lead.
        assert_eq!(note_lead_len("飲ん · 連用形"), 0);
        assert_eq!(note_lead_len(""), 0);
    }

    #[test]
    fn strip_pos_tag_drops_only_the_trailing_class_tag() {
        assert_eq!(
            strip_pos_tag("to drink; to gulp  (v5m)"),
            "to drink; to gulp"
        );
        assert_eq!(strip_pos_tag("topic marker  (prt)"), "topic marker");
        // A mid-gloss parenthetical (single space, not at the very end) stays.
        assert_eq!(
            strip_pos_tag("payload (of a packet, cell, etc.)  (n)"),
            "payload (of a packet, cell, etc.)"
        );
        // No tag: returned unchanged.
        assert_eq!(strip_pos_tag("morning"), "morning");
    }

    #[test]
    fn categorizes_by_part_of_speech() {
        assert_eq!(categorize(&verb_token()), WordCategory::Verb);
        assert_eq!(categorize(&tok("謎")), WordCategory::Unknown); // no entries
        let mut auxiliary = tok("ました");
        auxiliary.dictionary_form = "ます".to_string();
        auxiliary.source_pos = Some(LinderaPos::AuxVerb);
        assert_eq!(categorize(&auxiliary), WordCategory::Auxiliary);
    }

    #[test]
    fn word_rect_uses_center_midpoints_then_falls_back() {
        let box_rect = rect(0.0, 10.0, 120.0, 30.0);
        let centers = [15.0, 45.0, 75.0, 105.0];
        // Word covering chars [2,4): left = midpoint(45,75)=60, right = box right.
        let r = word_rect(box_rect, &centers, 4, 2, 4);
        assert!((r.x - 60.0).abs() < 0.01);
        assert!((r.right() - 120.0).abs() < 0.01);
        assert_eq!(r.y, 10.0);
        assert_eq!(r.height, 30.0);
        // No centres: even spacing (30px/char), chars [0,1) => 0..30.
        let r = word_rect(box_rect, &[], 4, 0, 1);
        assert!((r.x - 0.0).abs() < 0.01);
        assert!((r.right() - 30.0).abs() < 0.01);
    }

    fn sense(pos: &[&str], misc: &[&str], glosses: &[&str]) -> Sense {
        Sense {
            part_of_speech: pos.iter().map(|s| s.to_string()).collect(),
            misc: misc.iter().map(|s| s.to_string()).collect(),
            glosses: glosses.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn sense_ordering_demotes_archaic_senses() {
        let senses = vec![
            sense(&["n"], &[], &["common meaning"]),
            sense(&["n"], &["arch"], &["archaic meaning"]),
            sense(&["n"], &[], &["second common meaning"]),
        ];
        // Archaic sense sinks below both common ones; common order preserved.
        assert_eq!(
            ordered_sense_indices(&senses, SenseHint::default()),
            vec![0, 2, 1]
        );
    }

    #[test]
    fn sense_ordering_prefers_transitive_with_wo_context() {
        let senses = vec![
            sense(&["v5k", "vi"], &[], &["to open (intransitive)"]),
            sense(&["v5k", "vt"], &[], &["to open (transitive)"]),
        ];
        let transitive = SenseHint {
            transitive: Some(true),
        };
        assert_eq!(ordered_sense_indices(&senses, transitive), vec![1, 0]);
        // No hint: JMdict order preserved.
        assert_eq!(
            ordered_sense_indices(&senses, SenseHint::default()),
            vec![0, 1]
        );
    }

    #[test]
    fn sense_ordering_demotion_beats_transitivity() {
        // An archaic transitive sense must not leapfrog a common intransitive one.
        let senses = vec![
            sense(&["v5k", "vi"], &[], &["common intransitive"]),
            sense(&["v5k", "vt"], &["arch"], &["archaic transitive"]),
        ];
        let transitive = SenseHint {
            transitive: Some(true),
        };
        assert_eq!(ordered_sense_indices(&senses, transitive), vec![0, 1]);
    }

    #[test]
    fn transitivity_hint_detects_object_marker() {
        let tokens = vec![tok("水"), tok("を"), tok("飲む")];
        assert_eq!(
            transitivity_hint(&tokens, 2),
            SenseHint {
                transitive: Some(true)
            }
        );
        // No を before this token.
        assert_eq!(transitivity_hint(&tokens, 0), SenseHint::default());
    }

    #[test]
    fn transitivity_hint_stops_at_clause_boundary() {
        // を belongs to the previous clause (before 、) → no hint for the later verb.
        let tokens = vec![tok("本"), tok("を"), tok("買い"), tok("、"), tok("帰る")];
        assert_eq!(transitivity_hint(&tokens, 4), SenseHint::default());
    }

    fn tok(surface: &str) -> Token {
        Token {
            surface: surface.to_string(),
            dictionary_form: surface.to_string(),
            reasons: Vec::new(),
            entries: Vec::new(),
            source_pos: None,
            note_override: None,
        }
    }
}
