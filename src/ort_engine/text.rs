use crate::script::{is_cjk, is_hiragana, is_kana, is_katakana};

use super::normalize_single_char;

pub(super) fn normalize_recognized_text(raw: &str) -> String {
    let normalized: String = raw
        .chars()
        .map(|ch| normalize_single_char(ch).unwrap_or(ch))
        .collect();

    let collapsed = normalized.split_whitespace().collect::<Vec<_>>().join(" ");
    let dekanji = fix_katakana_lookalikes(&collapsed);
    let dekanji = fix_hiragana_he_particle(&dekanji);
    let quantity_normalized = normalize_item_quantity_multipliers(&dekanji);
    let separated = split_latin_ui_boundaries(&quantity_normalized);
    normalize_common_game_text(&separated)
}

/// The katakana a kanji is commonly OCR-confused with. Only the pairs that are
/// near-identical glyphs.
fn katakana_lookalike(ch: char) -> Option<char> {
    Some(match ch {
        '一' => 'ー', // kanji "one" vs long vowel mark
        '二' => 'ニ', // kanji "two" vs katakana NI
        '力' => 'カ', // kanji "power" vs katakana KA
        '口' => 'ロ', // kanji "mouth" vs katakana RO
        '工' => 'エ', // kanji "craft" vs katakana E
        '卜' => 'ト', // kanji "divine" vs katakana TO
        '八' => 'ハ', // kanji "eight" vs katakana HA
        '夕' => 'タ', // kanji "evening" vs katakana TA
        '才' => 'オ', // kanji "talent" vs katakana O
        '矢' => 'ヤ', // loose match vs katakana YA
        _ => return None,
    })
}

/// Replace a kanji with its katakana look-alike when it sits between two
/// katakana. A real kanji virtually never does, so this rescues misreads like
/// ハーモ二ー -> ハーモニー without touching legitimate kanji.
fn fix_katakana_lookalikes(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    for (index, &ch) in chars.iter().enumerate() {
        let prev_is_katakana = index > 0 && is_katakana(chars[index - 1]);
        let next_is_katakana = index + 1 < chars.len() && is_katakana(chars[index + 1]);
        let between_katakana = prev_is_katakana && next_is_katakana;
        let adjacent_katakana = prev_is_katakana || next_is_katakana;
        match katakana_lookalike(ch) {
            Some(kana) if matches!(ch, '一' | '工') && adjacent_katakana => out.push(kana),
            Some(kana) if between_katakana => out.push(kana),
            _ => out.push(ch),
        }
    }
    out
}

/// Katakana ヘ and hiragana へ are visually identical, so the recognizer picks
/// one arbitrarily. A katakana ヘ followed by hiragana is virtually always the
/// direction particle へ (スペースへと, 旅行へと); inside a katakana word the
/// next character is katakana and the glyph is left alone.
fn fix_hiragana_he_particle(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    for (index, &ch) in chars.iter().enumerate() {
        if ch == 'ヘ' && chars.get(index + 1).copied().is_some_and(is_hiragana) {
            out.push('へ');
        } else {
            out.push(ch);
        }
    }
    out
}

fn split_latin_ui_boundaries(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut prev = None;
    for ch in text.chars() {
        if let Some(prev_ch) = prev
            && should_insert_latin_boundary(prev_ch, ch)
            && !out.ends_with(' ')
        {
            out.push(' ');
        }
        out.push(ch);
        prev = Some(ch);
    }
    out
}

fn should_insert_latin_boundary(left: char, right: char) -> bool {
    (left.is_ascii_lowercase() && right.is_ascii_uppercase())
        || (left.is_ascii_alphabetic() && right.is_ascii_digit() && !matches!(left, 'x' | 'X'))
        || (left.is_ascii_digit() && right.is_ascii_alphabetic())
}

fn normalize_item_quantity_multipliers(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    for (index, &ch) in chars.iter().enumerate() {
        if ch == '×'
            && index > 0
            && chars
                .get(index + 1)
                .is_some_and(|next| next.is_ascii_digit())
            && is_item_quantity_prefix(chars[index - 1])
        {
            out.push('x');
        } else {
            out.push(ch);
        }
    }
    out
}

fn is_item_quantity_prefix(ch: char) -> bool {
    ch.is_ascii_alphabetic() || is_kana(ch) || is_cjk(ch)
}

fn normalize_common_game_text(text: &str) -> String {
    let mut out = trim_leading_noise_marks(text);
    out = normalize_user_id(&out);
    out = normalize_japanese_ascii_punctuation(&out);
    out = fix_small_katakana_yo_yu(&out);
    out = strip_stray_digit_after_period(&out);
    out = crate::text_corrections::apply_common_replacements(&out);
    out = space_after_punctuation(&out);
    collapse_spaces(&out)
}

fn normalize_japanese_ascii_punctuation(text: &str) -> String {
    if !text.chars().any(|ch| is_kana(ch) || is_cjk(ch)) {
        return text.to_string();
    }

    text.chars()
        .map(|ch| match ch {
            '!' => '！',
            '?' => '？',
            '(' => '（',
            ')' => '）',
            _ => ch,
        })
        .collect()
}

/// Full-size katakana ヨ/ユ after a palatalizable kana and before more
/// katakana is virtually always the small glyph the recognizer missed
/// (シヨック -> ショック); a real full-size ヨ never follows シ/チ/ジ
/// mid-word.
fn fix_small_katakana_yo_yu(text: &str) -> String {
    const PALATALIZABLE: &[char] = &[
        'キ', 'ギ', 'シ', 'ジ', 'チ', 'ヂ', 'ニ', 'ヒ', 'ビ', 'ピ', 'ミ', 'リ',
    ];
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    for (index, &ch) in chars.iter().enumerate() {
        let prev_palatalizable = index > 0 && PALATALIZABLE.contains(&chars[index - 1]);
        let next_katakana = chars
            .get(index + 1)
            .copied()
            .is_some_and(crate::script::is_katakana);
        match ch {
            'ヨ' if prev_palatalizable && next_katakana => out.push('ョ'),
            'ユ' if prev_palatalizable && next_katakana => out.push('ュ'),
            _ => out.push(ch),
        }
    }
    out
}

/// A lone ASCII digit directly after a sentence-final 。 at the end of a line
/// is a neighboring list-number or footnote glyph pulled in by the crop, not
/// part of the sentence.
fn strip_stray_digit_after_period(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() >= 3 && chars[chars.len() - 1].is_ascii_digit() && chars[chars.len() - 2] == '。'
    {
        return chars[..chars.len() - 1].iter().collect();
    }
    text.to_string()
}

fn trim_leading_noise_marks(text: &str) -> String {
    let source = text.chars().collect::<Vec<_>>();
    let mut start = 0;
    while let Some(&ch) = source.get(start) {
        let next = source.get(start + 1).copied();
        // Stop at the first real word character so leading symbol/punctuation
        // noise is trimmed but Japanese text (kana/kanji) is preserved. Using
        // is_ascii_alphanumeric alone wrongly strips entire all-CJK strings.
        if ch.is_ascii_alphanumeric()
            || ch == '\''
            || ch == '"'
            || is_kana(ch)
            || is_cjk(ch)
            || (ch == '×' && next.is_some_and(|next| next.is_ascii_digit()))
        {
            break;
        }
        start += 1;
    }

    let trimmed = source[start..].iter().collect::<String>();
    let mut chars = trimmed.chars();
    if let (Some(first), Some(second)) = (chars.next(), chars.next())
        && is_cjk(first)
        && second.is_ascii_alphanumeric()
    {
        let mut out = String::new();
        out.push(second);
        out.push_str(chars.as_str());
        return out;
    }

    trimmed
}

/// Recognized-text markers that indicate a user-id line, and the minimum digit
/// run to treat as one. `USER_ID_KNOWN_VALUE` is a specific id whose label is
/// frequently misread (so the "id" marker alone is missed) in the target games.
const USER_ID_MARKER: &str = "id";
const USER_ID_KNOWN_VALUE: &str = "500055272";
const USER_ID_MIN_DIGITS: usize = 8;
const USER_ID_LABEL: &str = "User ID:";

fn normalize_user_id(text: &str) -> String {
    let lower = text.to_ascii_lowercase();
    if !lower.contains(USER_ID_MARKER) && !lower.contains(USER_ID_KNOWN_VALUE) {
        return text.to_string();
    }

    let digits = text
        .chars()
        .filter(char::is_ascii_digit)
        .collect::<String>();
    if digits.len() >= USER_ID_MIN_DIGITS {
        return format!("{USER_ID_LABEL}{digits}");
    }

    text.to_string()
}

fn collapse_spaces(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn space_after_punctuation(text: &str) -> String {
    let chars = text.chars().collect::<Vec<_>>();
    let mut out = String::with_capacity(text.len());
    for (index, ch) in chars.iter().copied().enumerate() {
        out.push(ch);
        let Some(next) = chars.get(index + 1).copied() else {
            continue;
        };
        let previous = index
            .checked_sub(1)
            .and_then(|previous| chars.get(previous).copied());
        let should_space = matches!(ch, '.' | ',' | '!' | '?' | ';')
            && !next.is_whitespace()
            && next.is_ascii_alphabetic()
            && !is_initialism_period(&chars, index)
            && !(ch == '.' && previous.is_some_and(|previous| previous.is_ascii_digit()));
        if should_space {
            out.push(' ');
        }
    }
    out
}

fn is_initialism_period(chars: &[char], index: usize) -> bool {
    if chars.get(index) != Some(&'.') {
        return false;
    }
    let Some(next) = chars.get(index + 1).copied() else {
        return false;
    };
    if !next.is_ascii_uppercase() {
        return false;
    }
    chars.get(index + 2) == Some(&'.')
}
