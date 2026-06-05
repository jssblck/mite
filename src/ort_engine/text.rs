use crate::script::{is_cjk, is_kana, is_katakana};

use super::normalize_single_char;

pub(super) fn normalize_recognized_text(raw: &str) -> String {
    let normalized: String = raw
        .chars()
        .map(|ch| normalize_single_char(ch).unwrap_or(ch))
        .collect();

    let collapsed = normalized.split_whitespace().collect::<Vec<_>>().join(" ");
    let dekanji = fix_katakana_lookalikes(&collapsed);
    let separated = split_latin_ui_boundaries(&dekanji);
    normalize_common_game_text(&separated)
}

/// The katakana a kanji is commonly OCR-confused with. Only the pairs that are
/// near-identical glyphs.
fn katakana_lookalike(ch: char) -> Option<char> {
    Some(match ch {
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
        let between_katakana = index > 0
            && index + 1 < chars.len()
            && is_katakana(chars[index - 1])
            && is_katakana(chars[index + 1]);
        match katakana_lookalike(ch) {
            Some(kana) if between_katakana => out.push(kana),
            _ => out.push(ch),
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
        || (left.is_ascii_alphabetic() && right.is_ascii_digit())
        || (left.is_ascii_digit() && right.is_ascii_alphabetic())
}

fn normalize_common_game_text(text: &str) -> String {
    let mut out = trim_leading_noise_marks(text);
    out = normalize_user_id(&out);
    out = crate::text_corrections::apply_common_replacements(&out);
    out = space_after_punctuation(&out);
    collapse_spaces(&out)
}

fn trim_leading_noise_marks(text: &str) -> String {
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.peek().copied() {
        // Stop at the first real word character so leading symbol/punctuation
        // noise is trimmed but Japanese text (kana/kanji) is preserved. Using
        // is_ascii_alphanumeric alone wrongly strips entire all-CJK strings.
        if ch.is_ascii_alphanumeric() || ch == '\'' || ch == '"' || is_kana(ch) || is_cjk(ch) {
            break;
        }
        chars.next();
    }

    let trimmed = chars.collect::<String>();
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
            && !(ch == '.' && previous.is_some_and(|previous| previous.is_ascii_digit()));
        if should_space {
            out.push(' ');
        }
    }
    out
}
