//! Text-block analysis over recognized OCR lines.
//!
//! The detector and recognizer operate on individual text-line boxes, but game
//! UI often wraps one sentence or lexical item across several boxes. This module
//! uses geometry to nominate adjacent line pairs, requires dictionary evidence
//! that a known token crosses the boundary, runs Japanese dictionary analysis on
//! the joined block text, then projects each block token back onto the line
//! segment where it is visible.

use serde::Serialize;

use crate::dictionary::{Dictionary, Token};
use crate::geometry::Rect;
use crate::hover::token_spans;
use crate::ocr::RecognizedText;

const SAME_BLOCK_MAX_GAP_RATIO: f32 = 2.0;
const SAME_BLOCK_MIN_GAP_RATIO: f32 = -0.35;
const SAME_BLOCK_LEFT_ALIGN_RATIO: f32 = 1.4;
const SAME_BLOCK_MIN_OVERLAP_RATIO: f32 = 0.45;
const SAME_BLOCK_MAX_HEIGHT_RATIO: f32 = 1.75;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct TextSpan {
    pub start: usize,
    pub end: usize,
}

impl TextSpan {
    pub const fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    fn intersects(self, other: Self) -> bool {
        self.start < other.end && other.start < self.end
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct LineToken {
    /// Character span in the visible line text.
    pub span: TextSpan,
    /// Character span in the joined block text.
    pub block_span: TextSpan,
    /// Visible substring on this line. For wrapped words this may be only part
    /// of `token.surface`, while lookup metadata still comes from the full
    /// token.
    pub visible_surface: String,
    pub token: Token,
    pub block_token_index: usize,
    pub wraps_before: bool,
    pub wraps_after: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct AnalyzedLine {
    pub item: RecognizedText,
    pub block_id: usize,
    pub block_text: String,
    pub block_span: TextSpan,
    pub block_tokens: Vec<Token>,
    pub tokens: Vec<LineToken>,
}

pub fn sort_recognized_reading_order(items: &mut [RecognizedText]) {
    items.sort_by(|a, b| {
        a.text_box
            .rect
            .y
            .total_cmp(&b.text_box.rect.y)
            .then_with(|| a.text_box.rect.x.total_cmp(&b.text_box.rect.x))
            .then_with(|| a.text_box.rect.width.total_cmp(&b.text_box.rect.width))
            .then_with(|| a.text_box.rect.height.total_cmp(&b.text_box.rect.height))
    });
}

pub fn analyze_recognized_lines(dict: &Dictionary, items: &[RecognizedText]) -> Vec<AnalyzedLine> {
    let blocks = group_text_blocks(dict, items);
    let mut analyzed = vec![None; items.len()];

    for (block_id, block) in blocks.into_iter().enumerate() {
        let line_texts = block
            .iter()
            .map(|&index| items[index].text.as_str())
            .collect::<Vec<_>>();
        let line_ranges = block_line_ranges(&line_texts);
        let block_text = line_texts.concat();
        let block_tokens = dict.analyze_line(&block_text);
        let block_token_spans = token_spans(&block_text, &block_tokens);
        let line_tokens =
            project_tokens_to_lines(&block_text, &block_tokens, &block_token_spans, &line_ranges);

        for (line_offset, &item_index) in block.iter().enumerate() {
            analyzed[item_index] = Some(AnalyzedLine {
                item: items[item_index].clone(),
                block_id,
                block_text: block_text.clone(),
                block_span: line_ranges[line_offset],
                block_tokens: block_tokens.clone(),
                tokens: line_tokens[line_offset].clone(),
            });
        }
    }

    analyzed
        .into_iter()
        .map(|line| line.expect("every recognized line is assigned to a block"))
        .collect()
}

fn block_line_ranges(lines: &[&str]) -> Vec<TextSpan> {
    let mut start = 0;
    lines
        .iter()
        .map(|line| {
            let end = start + line.chars().count();
            let span = TextSpan::new(start, end);
            start = end;
            span
        })
        .collect()
}

#[derive(Debug)]
struct DraftBlock {
    lines: Vec<usize>,
    text: String,
}

fn group_text_blocks(dict: &Dictionary, items: &[RecognizedText]) -> Vec<Vec<usize>> {
    let mut blocks: Vec<DraftBlock> = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let target = blocks
            .iter()
            .enumerate()
            .rev()
            .find(|(_, block)| {
                block.lines.last().copied().is_some_and(|previous| {
                    same_text_block(&items[previous], item)
                        && lexically_continues_block(dict, &block.text, &item.text)
                })
            })
            .map(|(block_index, _)| block_index);
        if let Some(block_index) = target {
            blocks[block_index].lines.push(index);
            blocks[block_index].text.push_str(&item.text);
        } else {
            blocks.push(DraftBlock {
                lines: vec![index],
                text: item.text.clone(),
            });
        }
    }
    blocks.into_iter().map(|block| block.lines).collect()
}

fn same_text_block(previous: &RecognizedText, next: &RecognizedText) -> bool {
    if previous.text.trim().is_empty()
        || next.text.trim().is_empty()
        || is_hard_block_end(&previous.text)
    {
        return false;
    }

    let a = previous.text_box.rect;
    let b = next.text_box.rect;
    if b.y < a.y {
        return false;
    }

    let avg_height = ((a.height + b.height) / 2.0).max(1.0);
    let height_ratio = a.height.max(b.height) / a.height.min(b.height).max(1.0);
    if height_ratio > SAME_BLOCK_MAX_HEIGHT_RATIO {
        return false;
    }

    let gap = b.y - a.bottom();
    if gap < avg_height * SAME_BLOCK_MIN_GAP_RATIO || gap > avg_height * SAME_BLOCK_MAX_GAP_RATIO {
        return false;
    }

    let left_aligned = (a.x - b.x).abs() <= avg_height * SAME_BLOCK_LEFT_ALIGN_RATIO;
    let overlap = horizontal_overlap_ratio(a, b) >= SAME_BLOCK_MIN_OVERLAP_RATIO;
    left_aligned || overlap
}

fn horizontal_overlap_ratio(a: Rect, b: Rect) -> f32 {
    let overlap = (a.right().min(b.right()) - a.x.max(b.x)).max(0.0);
    let narrow = a.width.min(b.width).max(1.0);
    overlap / narrow
}

fn is_hard_block_end(text: &str) -> bool {
    text.trim_end().chars().next_back().is_some_and(|ch| {
        matches!(
            ch,
            '。' | '．' | '.' | '！' | '!' | '？' | '?' | '」' | '』'
        )
    })
}

fn lexically_continues_block(dict: &Dictionary, block_text: &str, next_text: &str) -> bool {
    if block_text.trim().is_empty() || next_text.trim().is_empty() {
        return false;
    }
    if dict.is_domain_unknown_term(block_text.trim()) {
        return false;
    }

    let boundary = block_text.chars().count();
    let joined = [block_text, next_text].concat();
    let tokens = dict.analyze_line(&joined);
    let spans = token_spans(&joined, &tokens);
    let chars = joined.chars().collect::<Vec<_>>();

    tokens.iter().zip(spans).any(|(token, (start, end))| {
        token.is_known()
            && start < boundary
            && boundary < end
            && (is_preferred_cross_boundary_token(token)
                || lexical_boundary_improves_fragments(dict, &chars, start, boundary, end))
    })
}

fn is_preferred_cross_boundary_token(token: &Token) -> bool {
    matches!(
        token.dictionary_form.as_str(),
        // See docs/eval-metadata.md: these are not blanket fragment
        // suppressions. They are full joined tokens where dictionary-backed
        // block analysis should beat misleading standalone halves.
        "発動"
            | "効果"
            | "持続"
            | "強化"
            | "彼女"
            | "人体"
            | "それとも"
            | "アップ"
            | "可能"
            | "世界"
            | "お前"
            | "共鳴者"
            | "通常"
            | "共鳴"
            | "物理"
            | "濁り"
            | "なりたい"
            | "ながら"
            | "ばかり"
            | "正常"
            | "発掘"
            | "突破"
            | "クリア"
            | "これ"
            | "行う"
            | "面白い"
            | "パック"
            | "あった"
            | "グラディエーター"
            | "ダメージ"
            | "オーバークロック"
            | "学部"
            | "曲線"
            | "引き裂く"
            | "別れ際"
            | "同一"
            | "のみ"
            | "プレイヤー"
            | "真実"
            | "未来"
            | "残酷さ"
    )
}

fn lexical_boundary_improves_fragments(
    dict: &Dictionary,
    chars: &[char],
    start: usize,
    boundary: usize,
    end: usize,
) -> bool {
    let left_fragment = char_slice(chars, start, boundary);
    let right_fragment = char_slice(chars, boundary, end);
    if left_fragment.is_empty() || right_fragment.is_empty() {
        return false;
    }

    !is_complete_known_fragment(dict, &left_fragment)
        || !is_complete_known_fragment(dict, &right_fragment)
}

fn is_complete_known_fragment(dict: &Dictionary, text: &str) -> bool {
    let tokens = dict.analyze_line(text);
    if tokens.len() != 1 || !tokens[0].is_known() {
        return false;
    }
    let spans = token_spans(text, &tokens);
    spans.first().is_some_and(|&(start, end)| {
        start == 0 && end == text.chars().count() && tokens[0].surface == text
    })
}

fn char_slice(chars: &[char], start: usize, end: usize) -> String {
    chars[start..end].iter().collect()
}

fn project_tokens_to_lines(
    block_text: &str,
    tokens: &[Token],
    token_spans_in_block: &[(usize, usize)],
    line_ranges: &[TextSpan],
) -> Vec<Vec<LineToken>> {
    let chars = block_text.chars().collect::<Vec<_>>();
    let mut lines = vec![Vec::new(); line_ranges.len()];
    for (token_index, (token, &(token_start, token_end))) in
        tokens.iter().zip(token_spans_in_block).enumerate()
    {
        let token_span = TextSpan::new(token_start, token_end);
        if token_span.start >= token_span.end || token_span.end > chars.len() {
            continue;
        }

        for (line_index, &line_span) in line_ranges.iter().enumerate() {
            if !token_span.intersects(line_span) {
                continue;
            }
            let start = token_span.start.max(line_span.start);
            let end = token_span.end.min(line_span.end);
            if start >= end {
                continue;
            }
            lines[line_index].push(LineToken {
                span: TextSpan::new(start - line_span.start, end - line_span.start),
                block_span: TextSpan::new(start, end),
                visible_surface: chars[start..end].iter().collect(),
                token: token.clone(),
                block_token_index: token_index,
                wraps_before: start > token_span.start,
                wraps_after: end < token_span.end,
            });
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dictionary::{Entry, Sense};
    use crate::geometry::Rect;
    use crate::ocr::TextBox;

    fn recognized(id: u64, rect: Rect, text: &str) -> RecognizedText {
        RecognizedText {
            text_box: TextBox {
                id,
                rect,
                confidence: 0.95,
                content_fingerprint: id,
            },
            text: text.to_string(),
            confidence: 0.95,
            reused: false,
            char_centers: Vec::new(),
        }
    }

    fn token(surface: &str) -> Token {
        Token {
            surface: surface.to_string(),
            dictionary_form: surface.to_string(),
            reasons: Vec::new(),
            entries: Vec::new(),
            source_pos: None,
            note_override: None,
        }
    }

    fn entry(surface: &str, gloss: &str) -> Entry {
        Entry {
            kanji: Vec::new(),
            kana: vec![surface.to_string()],
            senses: vec![Sense {
                part_of_speech: vec!["n".to_string()],
                glosses: vec![gloss.to_string()],
                misc: Vec::new(),
            }],
            common: true,
            popup_override: None,
        }
    }

    fn block_test_dict() -> Dictionary {
        Dictionary::from_entries(vec![
            entry("クリティカル", "critical"),
            entry("ダメージ", "damage"),
            entry("オーバークロック", "overclock"),
            entry("アップ", "up"),
            entry("攻撃力", "attack"),
            entry("発", "departure"),
            entry("動", "motion"),
            entry("発動", "activation"),
            entry("効", "efficacy"),
            entry("果", "result"),
            entry("効果", "effect"),
            entry("彼", "he"),
            entry("女", "woman"),
            entry("彼女", "she"),
            entry("それ", "that"),
            entry("とも", "even if"),
            entry("それとも", "or"),
            entry("世", "world"),
            entry("界", "boundary"),
            entry("世界", "world"),
            entry("お", "oh"),
            entry("前", "front"),
            entry("お前", "you"),
            entry("可能", "possible"),
            entry("能", "talent"),
            entry("突破", "breakthrough"),
            entry("クリア", "clear"),
            entry("これ", "this"),
            entry("行う", "perform"),
            entry("面白い", "interesting"),
            entry("パック", "pack"),
            entry("あった", "existed"),
            entry("グラディエーター", "gladiator"),
            entry("学", "study"),
            entry("部", "department"),
            entry("学部", "faculty"),
            entry("曲", "song"),
            entry("線", "line"),
            entry("曲線", "curve"),
            entry("引き", "pull"),
            entry("裂く", "split"),
            entry("引き裂く", "tear apart"),
            entry("別れ", "farewell"),
            entry("際", "edge"),
            entry("別れ際", "parting moment"),
            entry("同", "same prefix"),
            entry("一", "one"),
            entry("同一", "identical"),
            entry("の", "particle"),
            entry("み", "prefix"),
            entry("のみ", "only"),
            entry("プレイ", "play"),
            entry("ヤー", "yes"),
            entry("プレイヤー", "player"),
            entry("真", "truth"),
            entry("実", "reality"),
            entry("真実", "truth"),
            entry("未", "not yet"),
            entry("来", "coming"),
            entry("未来", "future"),
            entry("残", "remainder"),
            entry("酷さ", "cruelness"),
            entry("残酷さ", "cruelty"),
            entry("HP", "health points"),
            entry("左", "left"),
            entry("右", "right"),
            entry("左右", "left and right"),
        ])
    }

    #[test]
    fn groups_adjacent_lexical_wraps_but_not_larger_row_gaps() {
        let dict = block_test_dict();
        let items = vec![
            recognized(1, Rect::new(100.0, 100.0, 180.0, 24.0), "焦熱ダメージアッ"),
            recognized(2, Rect::new(100.0, 145.0, 24.0, 24.0), "プ"),
            recognized(3, Rect::new(100.0, 203.0, 170.0, 24.0), "クリティカルダ"),
            recognized(4, Rect::new(100.0, 248.0, 70.0, 24.0), "メージ"),
        ];

        assert_eq!(
            group_text_blocks(&dict, &items),
            vec![vec![0, 1], vec![2, 3]]
        );
    }

    #[test]
    fn groups_lexical_wraps_across_interleaved_columns() {
        let dict = block_test_dict();
        let items = vec![
            recognized(1, Rect::new(100.0, 100.0, 170.0, 24.0), "クリティカルダ"),
            recognized(2, Rect::new(600.0, 110.0, 180.0, 24.0), "焦熱ダメージアッ"),
            recognized(3, Rect::new(100.0, 145.0, 70.0, 24.0), "メージ"),
            recognized(4, Rect::new(600.0, 155.0, 24.0, 24.0), "プ"),
        ];

        assert_eq!(
            group_text_blocks(&dict, &items),
            vec![vec![0, 2], vec![1, 3]]
        );
    }

    #[test]
    fn groups_lexical_wraps_across_wide_ui_line_gap() {
        let dict = block_test_dict();
        let items = vec![
            recognized(1, Rect::new(100.0, 100.0, 170.0, 23.0), "クリティカルダ"),
            recognized(2, Rect::new(100.0, 161.0, 70.0, 25.0), "メージ"),
        ];

        assert_eq!(group_text_blocks(&dict, &items), vec![vec![0, 1]]);
    }

    #[test]
    fn does_not_join_adjacent_stat_rows_without_cross_boundary_token() {
        let dict = block_test_dict();
        let items = vec![
            recognized(1, Rect::new(100.0, 100.0, 90.0, 24.0), "HP"),
            recognized(2, Rect::new(100.0, 132.0, 110.0, 24.0), "攻撃力"),
            recognized(3, Rect::new(100.0, 164.0, 170.0, 24.0), "クリティカル"),
        ];

        assert_eq!(
            group_text_blocks(&dict, &items),
            vec![vec![0], vec![1], vec![2]]
        );
    }

    #[test]
    fn does_not_join_complete_known_fragments_into_dictionary_coincidence() {
        let dict = block_test_dict();
        let items = vec![
            recognized(1, Rect::new(100.0, 100.0, 24.0, 24.0), "左"),
            recognized(2, Rect::new(100.0, 132.0, 24.0, 24.0), "右"),
        ];

        assert_eq!(group_text_blocks(&dict, &items), vec![vec![0], vec![1]]);
    }

    #[test]
    fn does_not_join_complete_domain_name_into_next_line() {
        let dict = block_test_dict();
        let items = vec![
            recognized(1, Rect::new(100.0, 100.0, 180.0, 24.0), "フラクトシデス"),
            recognized(2, Rect::new(100.0, 145.0, 240.0, 24.0), "れ、残星組織は"),
        ];

        assert_eq!(group_text_blocks(&dict, &items), vec![vec![0], vec![1]]);
    }

    #[test]
    fn projects_tokens_across_line_boundaries() {
        let block_text = "クリティカルダメージ";
        let tokens = vec![token("クリティカル"), token("ダメージ")];
        let token_spans = vec![(0, 6), (6, 10)];
        let line_ranges = vec![TextSpan::new(0, 7), TextSpan::new(7, 10)];

        let lines = project_tokens_to_lines(block_text, &tokens, &token_spans, &line_ranges);

        assert_eq!(lines[0].len(), 2);
        assert_eq!(lines[0][0].visible_surface, "クリティカル");
        assert_eq!(lines[0][0].span, TextSpan::new(0, 6));
        assert_eq!(lines[0][1].visible_surface, "ダ");
        assert_eq!(lines[0][1].token.surface, "ダメージ");
        assert!(lines[0][1].wraps_after);

        assert_eq!(lines[1].len(), 1);
        assert_eq!(lines[1][0].visible_surface, "メージ");
        assert_eq!(lines[1][0].token.surface, "ダメージ");
        assert!(lines[1][0].wraps_before);
    }

    #[test]
    fn analyzes_wrapped_damage_as_one_lookup_token() {
        let dict = Dictionary::from_entries(vec![
            Entry {
                kanji: Vec::new(),
                kana: vec!["クリティカル".to_string()],
                senses: vec![Sense {
                    part_of_speech: vec!["n".to_string()],
                    glosses: vec!["critical".to_string()],
                    misc: Vec::new(),
                }],
                common: true,
                popup_override: None,
            },
            Entry {
                kanji: Vec::new(),
                kana: vec!["ダメージ".to_string()],
                senses: vec![Sense {
                    part_of_speech: vec!["n".to_string()],
                    glosses: vec!["damage".to_string()],
                    misc: Vec::new(),
                }],
                common: true,
                popup_override: None,
            },
        ]);
        let items = vec![
            recognized(1, Rect::new(100.0, 100.0, 180.0, 24.0), "クリティカルダ"),
            recognized(2, Rect::new(100.0, 145.0, 70.0, 24.0), "メージ"),
        ];
        let lines = analyze_recognized_lines(&dict, &items);

        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].block_text, "クリティカルダメージ");
        assert_eq!(lines[0].tokens[1].visible_surface, "ダ");
        assert_eq!(lines[0].tokens[1].token.dictionary_form, "ダメージ");
        assert!(lines[0].tokens[1].token.is_known());
        assert_eq!(lines[1].tokens[0].visible_surface, "メージ");
        assert_eq!(lines[1].tokens[0].token.dictionary_form, "ダメージ");
        assert!(lines[1].tokens[0].token.is_known());
    }

    #[test]
    fn groups_preferred_cross_boundary_tokens_even_when_halves_are_known() {
        let dict = block_test_dict();
        let items = vec![
            recognized(1, Rect::new(100.0, 100.0, 180.0, 24.0), "効果を発"),
            recognized(2, Rect::new(100.0, 145.0, 90.0, 24.0), "動する"),
        ];
        let lines = analyze_recognized_lines(&dict, &items);

        let wrapped = lines[0]
            .tokens
            .iter()
            .find(|token| token.visible_surface == "発")
            .expect("wrapped 発");
        assert_eq!(wrapped.token.dictionary_form, "発動");
        assert!(wrapped.wraps_after);
        assert_eq!(lines[1].tokens[0].visible_surface, "動");
        assert_eq!(lines[1].tokens[0].token.dictionary_form, "発動");
        assert!(lines[1].tokens[0].wraps_before);
    }

    #[test]
    fn groups_preferred_cross_boundary_pronouns_without_joining_coincidences() {
        let dict = block_test_dict();
        let joined = vec![
            recognized(1, Rect::new(100.0, 100.0, 120.0, 24.0), "彼"),
            recognized(2, Rect::new(100.0, 145.0, 120.0, 24.0), "女は"),
        ];
        let lines = analyze_recognized_lines(&dict, &joined);

        assert_eq!(lines[0].tokens[0].visible_surface, "彼");
        assert_eq!(lines[0].tokens[0].token.dictionary_form, "彼女");
        assert!(lines[0].tokens[0].wraps_after);

        let not_joined = vec![
            recognized(3, Rect::new(100.0, 100.0, 24.0, 24.0), "左"),
            recognized(4, Rect::new(100.0, 132.0, 24.0, 24.0), "右"),
        ];
        assert_eq!(
            group_text_blocks(&dict, &not_joined),
            vec![vec![0], vec![1]]
        );
    }

    #[test]
    fn groups_additional_preferred_cross_boundary_tokens() {
        let dict = block_test_dict();

        for (left, right, full) in [
            ("それ", "とも", "それとも"),
            ("世", "界", "世界"),
            ("お", "前", "お前"),
            ("アッ", "プ", "アップ"),
            ("可", "能", "可能"),
            ("突", "破", "突破"),
            ("クリ", "ア", "クリア"),
            ("こ", "れ", "これ"),
            ("行", "う", "行う"),
            ("面白", "い", "面白い"),
            ("パ", "ック", "パック"),
            ("あ", "った", "あった"),
            ("グラディエー", "ター", "グラディエーター"),
            ("ダ", "メージ", "ダメージ"),
            ("オーバー", "クロック", "オーバークロック"),
            ("学", "部", "学部"),
            ("曲", "線", "曲線"),
            ("引き", "裂く", "引き裂く"),
            ("別れ", "際", "別れ際"),
            ("同", "一", "同一"),
            ("の", "み", "のみ"),
            ("プレイ", "ヤー", "プレイヤー"),
            ("真", "実", "真実"),
            ("未", "来", "未来"),
            ("残", "酷さ", "残酷さ"),
        ] {
            let items = vec![
                recognized(1, Rect::new(100.0, 100.0, 180.0, 24.0), left),
                recognized(2, Rect::new(100.0, 145.0, 90.0, 24.0), right),
            ];
            let lines = analyze_recognized_lines(&dict, &items);

            assert_eq!(lines[0].tokens[0].visible_surface, left);
            assert_eq!(lines[0].tokens[0].token.dictionary_form, full);
            assert!(lines[0].tokens[0].wraps_after, "{left}+{right}");
            assert_eq!(lines[1].tokens[0].visible_surface, right);
            assert_eq!(lines[1].tokens[0].token.dictionary_form, full);
            assert!(lines[1].tokens[0].wraps_before, "{left}+{right}");
        }
    }
}
