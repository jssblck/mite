//! Friendly part-of-speech labels for the popup's category pill.
//!
//! The popup heads each definition with a small colour-matched pill naming the
//! word's grammatical class in plain terms (`NOUN`, `I-ADJECTIVE`,
//! `VERB (GODAN)`). The colour comes from [`WordCategory`]; the text refines it
//! with the conjugation/inflection subclass JMdict already encodes in its POS
//! abbreviations, so a learner sees *how* a verb or adjective conjugates without
//! decoding `v5m`/`adj-i` tags.

use crate::dictionary::Token;

use super::{WordCategory, categorize};

/// Plain-language pill label for a token, or `None` when there is nothing useful
/// to name (uncategorised or unknown words). The label is uppercase so it reads
/// as a compact tag rather than prose.
pub fn pill_label(token: &Token) -> Option<String> {
    let pos = token
        .entries
        .first()
        .and_then(|entry| entry.senses.first())
        .map(|sense| sense.part_of_speech.as_slice())
        .unwrap_or(&[]);

    match categorize(token) {
        WordCategory::Noun => Some("NOUN".to_string()),
        WordCategory::Particle => Some("PARTICLE".to_string()),
        WordCategory::Adverb => Some("ADVERB".to_string()),
        WordCategory::Expression => Some("EXPRESSION".to_string()),
        WordCategory::Auxiliary => Some("AUXILIARY".to_string()),
        WordCategory::Verb => Some(match verb_subclass(pos) {
            Some(sub) => format!("VERB ({sub})"),
            None => "VERB".to_string(),
        }),
        WordCategory::Adjective => Some(adjective_label(pos).to_string()),
        WordCategory::Other => other_label(pos).map(str::to_string),
        WordCategory::Unknown => None,
    }
}

/// Conjugation class of a verb from its JMdict POS tags, in the wording a
/// learner recognises (godan/ichidan/suru/kuru). `None` for verb tags with no
/// modern conjugation class (e.g. classical 二段/四段 or a bare `vi`/`vt`).
fn verb_subclass(pos: &[String]) -> Option<&'static str> {
    pos.iter().find_map(|tag| {
        let tag = tag.as_str();
        if tag == "vk" {
            Some("KURU")
        } else if tag.starts_with("vs") || tag == "vz" {
            // vs, vs-s, vs-i (suru verbs) and vz (zuru → suru class).
            Some("SURU")
        } else if tag.starts_with("v1") {
            Some("ICHIDAN")
        } else if tag.starts_with("v5") {
            Some("GODAN")
        } else {
            None
        }
    })
}

/// Adjective subclass label (i-adjective vs na-adjective), defaulting to a bare
/// `ADJECTIVE` for the rarer adjectival classes.
fn adjective_label(pos: &[String]) -> &'static str {
    for tag in pos {
        match tag.as_str() {
            "adj-i" | "adj-ix" => return "I-ADJECTIVE",
            "adj-na" | "adj-nari" => return "NA-ADJECTIVE",
            _ => {}
        }
    }
    "ADJECTIVE"
}

/// Friendly labels for the grammatical classes that fall under
/// [`WordCategory::Other`]; `None` when no specific tag is recognised, so the
/// popup simply omits the pill rather than showing a meaningless one.
fn other_label(pos: &[String]) -> Option<&'static str> {
    pos.iter().find_map(|tag| match tag.as_str() {
        "conj" => Some("CONJUNCTION"),
        "int" => Some("INTERJECTION"),
        "pref" => Some("PREFIX"),
        "suf" => Some("SUFFIX"),
        "ctr" => Some("COUNTER"),
        "num" => Some("NUMBER"),
        "adj-pn" => Some("PRENOMINAL"),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dictionary::{Entry, Sense};

    fn token(pos: &[&str]) -> Token {
        Token {
            surface: "x".to_string(),
            dictionary_form: "x".to_string(),
            reasons: Vec::new(),
            entries: vec![Entry {
                kanji: Vec::new(),
                kana: Vec::new(),
                senses: vec![Sense {
                    part_of_speech: pos.iter().map(|s| s.to_string()).collect(),
                    glosses: vec!["g".to_string()],
                    misc: Vec::new(),
                }],
                common: true,
                popup_override: None,
            }],
            source_pos: None,
            note_override: None,
        }
    }

    #[test]
    fn names_verb_conjugation_classes() {
        assert_eq!(
            pill_label(&token(&["v5m"])).as_deref(),
            Some("VERB (GODAN)")
        );
        assert_eq!(
            pill_label(&token(&["v1"])).as_deref(),
            Some("VERB (ICHIDAN)")
        );
        assert_eq!(
            pill_label(&token(&["vs-i"])).as_deref(),
            Some("VERB (SURU)")
        );
        assert_eq!(pill_label(&token(&["vk"])).as_deref(), Some("VERB (KURU)"));
        // A verb tag with no modern conjugation class drops the qualifier.
        assert_eq!(pill_label(&token(&["v2k-s"])).as_deref(), Some("VERB"));
    }

    #[test]
    fn names_adjective_classes() {
        assert_eq!(
            pill_label(&token(&["adj-i"])).as_deref(),
            Some("I-ADJECTIVE")
        );
        assert_eq!(
            pill_label(&token(&["adj-na"])).as_deref(),
            Some("NA-ADJECTIVE")
        );
        assert_eq!(
            pill_label(&token(&["adj-no"])).as_deref(),
            Some("ADJECTIVE")
        );
    }

    #[test]
    fn names_simple_classes_and_omits_unknown() {
        assert_eq!(pill_label(&token(&["n"])).as_deref(), Some("NOUN"));
        assert_eq!(pill_label(&token(&["prt"])).as_deref(), Some("PARTICLE"));
        assert_eq!(
            pill_label(&token(&["conj"])).as_deref(),
            Some("CONJUNCTION")
        );
        // Unrecognised "other" tag → no pill.
        assert_eq!(pill_label(&token(&["unc"])), None);
        // No entries at all → unknown → no pill.
        let mut bare = token(&["n"]);
        bare.entries.clear();
        assert_eq!(pill_label(&bare), None);
    }
}
