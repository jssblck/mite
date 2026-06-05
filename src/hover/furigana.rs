use serde::{Deserialize, Serialize};

use crate::script::is_cjk;

/// One piece of a headword for ruby rendering: a run of text and, when that run
/// is kanji, the furigana to draw above it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FuriSegment {
    pub text: String,
    pub furigana: Option<String>,
}

/// Split a kanji-bearing headword into ruby segments, distributing `reading`
/// over the kanji runs (okurigana matching): each maximal kanji run gets the
/// reading between its surrounding kana, which appear literally. Falls back to
/// the whole reading over the whole word if the kana don't line up.
pub fn furigana_segments(word: &str, reading: &str) -> Vec<FuriSegment> {
    let whole = || {
        vec![FuriSegment {
            text: word.to_string(),
            furigana: Some(reading.to_string()),
        }]
    };

    // Runs of (is_kanji, text).
    let mut runs: Vec<(bool, String)> = Vec::new();
    for ch in word.chars() {
        let kanji = is_kanji_char(ch);
        match runs.last_mut() {
            Some(last) if last.0 == kanji => last.1.push(ch),
            _ => runs.push((kanji, ch.to_string())),
        }
    }

    let reading: Vec<char> = reading.chars().collect();
    let mut segments = Vec::with_capacity(runs.len());
    let mut pos = 0usize;
    for (index, (kanji, text)) in runs.iter().enumerate() {
        if !kanji {
            // Kana run must appear literally at the current reading position.
            let chars: Vec<char> = text.chars().collect();
            if pos + chars.len() <= reading.len() && reading[pos..pos + chars.len()] == chars[..] {
                segments.push(FuriSegment {
                    text: text.clone(),
                    furigana: None,
                });
                pos += chars.len();
            } else {
                return whole();
            }
        } else {
            // Kanji run owns the reading up to where the next kana run matches.
            let next_kana: Option<Vec<char>> = runs[index + 1..]
                .iter()
                .find(|(k, _)| !k)
                .map(|(_, t)| t.chars().collect());
            let end = match &next_kana {
                Some(kana) => match find_subslice(&reading, pos, kana) {
                    Some(at) => at,
                    None => return whole(),
                },
                None => reading.len(),
            };
            if end < pos {
                return whole();
            }
            let furigana: String = reading[pos..end].iter().collect();
            segments.push(FuriSegment {
                text: text.clone(),
                furigana: (!furigana.is_empty()).then_some(furigana),
            });
            pos = end;
        }
    }

    if pos == reading.len() {
        segments
    } else {
        whole()
    }
}

/// First index `>= from` where `needle` matches `haystack`.
fn find_subslice(haystack: &[char], from: usize, needle: &[char]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    (from..=haystack.len() - needle.len())
        .find(|&start| haystack[start..start + needle.len()] == *needle)
}

fn is_kanji_char(ch: char) -> bool {
    // Kanji are exactly the CJK ideograph blocks; reuse the shared predicate so
    // the block ranges live in one place.
    is_cjk(ch)
}

pub(super) fn has_kanji(text: &str) -> bool {
    text.chars().any(is_kanji_char)
}
