//! Unicode script predicates for Japanese OCR text.
//!
//! Recognizer post-processing and recognized-item filtering both need to know
//! whether a character is CJK or kana; keeping the block ranges in one place
//! avoids the predicates drifting apart between modules.

/// CJK unified ideographs block (Extension A start through the main block end).
const CJK_IDEOGRAPH_START: char = '\u{3400}';
const CJK_IDEOGRAPH_END: char = '\u{9fff}';
/// CJK compatibility ideographs block.
const CJK_COMPAT_START: char = '\u{f900}';
const CJK_COMPAT_END: char = '\u{faff}';
/// Combined hiragana + katakana span (katakana includes the prolonged sound
/// mark ー, U+30FC).
const KANA_START: char = '\u{3040}';
const KANA_END: char = '\u{30ff}';
/// Katakana block (also includes the prolonged sound mark ー, U+30FC).
const KATAKANA_START: char = '\u{30a0}';
const KATAKANA_END: char = '\u{30ff}';

/// CJK unified ideographs (kanji), including Extension A and the compatibility
/// ideographs block.
pub fn is_cjk(ch: char) -> bool {
    (CJK_IDEOGRAPH_START..=CJK_IDEOGRAPH_END).contains(&ch)
        || (CJK_COMPAT_START..=CJK_COMPAT_END).contains(&ch)
}

/// Hiragana and katakana (including the prolonged sound mark ー, U+30FC).
pub fn is_kana(ch: char) -> bool {
    (KANA_START..=KANA_END).contains(&ch)
}

/// Katakana block (includes the prolonged sound mark ー, U+30FC).
pub fn is_katakana(ch: char) -> bool {
    (KATAKANA_START..=KATAKANA_END).contains(&ch)
}

/// Hiragana block only (excludes katakana).
pub fn is_hiragana(ch: char) -> bool {
    ('\u{3040}'..'\u{30a0}').contains(&ch)
}
