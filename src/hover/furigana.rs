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

/// Ruby for the on-screen *surface* form, derived from the dictionary-form ruby.
///
/// The overlay draws furigana over the glyphs the game actually renders (the
/// surface), but [`furigana_segments`] keys its reading to the dictionary form.
/// Japanese inflection only rewrites trailing okurigana, so the kanji stem and
/// its reading are invariant: each kanji run in the surface reuses the reading
/// of the matching kanji run from `dict_ruby` (matched by identical text,
/// scanning forward in order, so a wrapped surface that begins partway through
/// the word still aligns to the right dictionary run). Surface kana runs carry
/// no furigana. This never invents a reading: a surface kanji run with no
/// matching dictionary run is left bare.
pub fn surface_furigana(surface: &str, dict_ruby: &[FuriSegment]) -> Vec<FuriSegment> {
    // Kanji runs of the dictionary ruby that carry a reading, in order.
    let dict_kanji: Vec<&FuriSegment> = dict_ruby
        .iter()
        .filter(|seg| seg.furigana.is_some() && has_kanji(&seg.text))
        .collect();

    let mut next = 0usize;
    let mut segments: Vec<FuriSegment> = Vec::new();
    for ch in surface.chars() {
        let kanji = is_kanji_char(ch);
        match segments.last_mut() {
            // Extend the current run when its kanji/kana-ness matches.
            Some(last) if is_kanji_char_run(&last.text) == kanji => last.text.push(ch),
            _ => segments.push(FuriSegment {
                text: ch.to_string(),
                furigana: None,
            }),
        }
    }

    for segment in &mut segments {
        if !is_kanji_char_run(&segment.text) {
            continue;
        }
        // Scan forward from the last match for a dictionary kanji run with
        // identical text. The visible surface may begin partway through the word
        // (a wrapped tail), so the first surface kanji run need not be the first
        // dictionary run; forward scanning keeps repeated kanji aligned in order.
        if let Some(offset) = dict_kanji[next..]
            .iter()
            .position(|dict| dict.text == segment.text)
        {
            segment.furigana = dict_kanji[next + offset].furigana.clone();
            next += offset + 1;
        }
    }
    segments
}

/// Whether a run (built one char at a time above) is kanji. Empty runs are kana.
fn is_kanji_char_run(text: &str) -> bool {
    text.chars().next().is_some_and(is_kanji_char)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(text: &str, furi: Option<&str>) -> FuriSegment {
        FuriSegment {
            text: text.to_string(),
            furigana: furi.map(str::to_string),
        }
    }

    #[test]
    fn surface_ruby_reuses_stem_reading_across_inflection() {
        // Dictionary 飲む / のむ → 飲[の] + む. The surface 飲みます keeps the 飲
        // stem reading and leaves its kana okurigana bare.
        let dict = furigana_segments("飲む", "のむ");
        assert_eq!(
            surface_furigana("飲みます", &dict),
            vec![seg("飲", Some("の")), seg("みます", None)],
        );
    }

    #[test]
    fn surface_ruby_matches_uninflected_word() {
        let dict = furigana_segments("寒い", "さむい");
        assert_eq!(
            surface_furigana("寒い", &dict),
            vec![seg("寒", Some("さむ")), seg("い", None)],
        );
        let dict = furigana_segments("母", "はは");
        assert_eq!(surface_furigana("母", &dict), vec![seg("母", Some("はは"))]);
    }

    #[test]
    fn surface_ruby_handles_compound_kanji_runs_in_order() {
        // 取り出す / とりだす → 取[と] り 出[だ] す; an inflected surface keeps both
        // stem readings positionally.
        let dict = furigana_segments("取り出す", "とりだす");
        assert_eq!(
            surface_furigana("取り出した", &dict),
            vec![
                seg("取", Some("と")),
                seg("り", None),
                seg("出", Some("だ")),
                seg("した", None),
            ],
        );
    }

    #[test]
    fn surface_ruby_aligns_a_wrapped_tail_to_a_later_kanji_run() {
        // The visible surface begins partway through the word: only 出した shows
        // on this line (取り wrapped off the previous one). The 出 run must still
        // pick up its reading by scanning forward past the unseen 取 run.
        let dict = furigana_segments("取り出す", "とりだす");
        assert_eq!(
            surface_furigana("出した", &dict),
            vec![seg("出", Some("だ")), seg("した", None)],
        );
    }

    #[test]
    fn surface_ruby_leaves_kana_only_surface_bare() {
        let dict = furigana_segments("ゆっくり", "ゆっくり");
        assert_eq!(
            surface_furigana("ゆっくり", &dict),
            vec![seg("ゆっくり", None)],
        );
    }

    #[test]
    fn surface_ruby_leaves_unmatched_kanji_bare() {
        // A surface kanji run that doesn't match the dictionary stem never gets a
        // borrowed (wrong) reading.
        let dict = furigana_segments("食べる", "たべる");
        assert_eq!(
            surface_furigana("飲む", &dict),
            vec![seg("飲", None), seg("む", None)]
        );
    }
}
