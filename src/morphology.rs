//! Japanese morphological analysis via Lindera (embedded IPADIC).
//!
//! This replaces the earlier hand-rolled longest-match segmentation and rule
//! table: Lindera does cost-based Viterbi segmentation and gives each morpheme
//! its dictionary (base) form, reading, part of speech, and conjugation form.
//! The base form is what we then look up in JMdict for glosses.

use anyhow::{Result, anyhow};
use lindera::dictionary::load_dictionary;
use lindera::mode::Mode;
use lindera::segmenter::Segmenter;
use lindera::tokenizer::Tokenizer;

use crate::pos::LinderaPos;

/// IPADIC feature (detail) indices in the comma-separated MeCab field order.
const POS_FIELDS: std::ops::Range<usize> = 0..4;
const CONJUGATION_FORM: usize = 5;
const BASE_FORM: usize = 6;
const READING: usize = 7;

/// Embedded dictionary URI passed to Lindera.
const EMBEDDED_IPADIC_URI: &str = "embedded://ipadic";
/// MeCab feature placeholders that mean "no value" for a field.
const FIELD_PLACEHOLDERS: [&str; 2] = ["*", "UNK"];

/// One analyzed morpheme.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Morpheme {
    /// Exact surface substring as it appeared in the input.
    pub surface: String,
    /// Dictionary/base form (IPADIC 原形); falls back to `surface` when the
    /// morpheme is unknown or carries no base form.
    pub base_form: String,
    /// Katakana reading (IPADIC 読み), if known.
    pub reading: Option<String>,
    /// Part-of-speech fields, major first (IPADIC 品詞 + subcategories),
    /// dropping the `*` placeholders.
    pub pos: Vec<String>,
    /// Conjugation form (IPADIC 活用形, e.g. 連用形), if inflected.
    pub conjugation_form: Option<String>,
}

impl Morpheme {
    /// Whether this morpheme is inflected (surface differs from base form).
    pub fn is_inflected(&self) -> bool {
        self.surface != self.base_form
    }

    /// IPADIC major part of speech (品詞 大分類).
    pub fn major_pos(&self) -> LinderaPos {
        LinderaPos::from_major(self.pos.first().map(String::as_str).unwrap_or(""))
    }
}

/// A reusable Lindera tokenizer over the embedded IPADIC dictionary.
pub struct Analyzer {
    tokenizer: Tokenizer,
}

impl Analyzer {
    /// Build an analyzer backed by the embedded IPADIC dictionary.
    pub fn new() -> Result<Self> {
        let dictionary = load_dictionary(EMBEDDED_IPADIC_URI)
            .map_err(|error| anyhow!("failed to load embedded IPADIC dictionary: {error}"))?;
        let segmenter = Segmenter::new(Mode::Normal, dictionary, None);
        let tokenizer = Tokenizer::new(segmenter);
        Ok(Self { tokenizer })
    }

    /// Segment a line into morphemes. Whitespace-only tokens are dropped.
    pub fn analyze(&self, line: &str) -> Result<Vec<Morpheme>> {
        let tokens = self
            .tokenizer
            .tokenize(line)
            .map_err(|error| anyhow!("lindera tokenize failed: {error}"))?;

        let mut morphemes = Vec::with_capacity(tokens.len());
        for mut token in tokens {
            let surface = token.surface.to_string();
            if surface.trim().is_empty() {
                continue;
            }
            let details = token.details();
            let field = |index: usize| {
                details
                    .get(index)
                    .copied()
                    .filter(|value| !value.is_empty() && !FIELD_PLACEHOLDERS.contains(value))
                    .map(str::to_string)
            };

            let base_form = field(BASE_FORM).unwrap_or_else(|| surface.clone());
            let reading = field(READING);
            let conjugation_form = field(CONJUGATION_FORM);
            let pos = POS_FIELDS.filter_map(field).collect();

            morphemes.push(Morpheme {
                surface,
                base_form,
                reading,
                pos,
                conjugation_form,
            });
        }
        Ok(morphemes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segments_and_lemmatizes_inflected_verb() {
        let analyzer = Analyzer::new().expect("load IPADIC");
        let morphemes = analyzer.analyze("水を飲みました").expect("analyze");
        let surfaces: Vec<&str> = morphemes.iter().map(|m| m.surface.as_str()).collect();
        // 水 / を / 飲み / まし / た  (IPADIC short units)
        assert_eq!(surfaces.first().copied(), Some("水"));
        assert!(surfaces.contains(&"を"));

        // The verb morpheme lemmatizes 飲み -> 飲む.
        let verb = morphemes
            .iter()
            .find(|m| m.surface == "飲み")
            .expect("verb morpheme present");
        assert_eq!(verb.base_form, "飲む");
        assert!(verb.is_inflected());
    }

    #[test]
    fn keeps_uninflected_noun_base_form() {
        let analyzer = Analyzer::new().expect("load IPADIC");
        let morphemes = analyzer.analyze("学校").expect("analyze");
        assert_eq!(morphemes.len(), 1);
        assert_eq!(morphemes[0].surface, "学校");
        assert_eq!(morphemes[0].base_form, "学校");
        assert!(!morphemes[0].is_inflected());
    }
}
