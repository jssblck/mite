//! jmdict-simplified deserialization boundary.

use serde::Deserialize;

use super::{Entry, Sense};

#[derive(Debug, Deserialize)]
pub(super) struct RawWord {
    #[serde(default)]
    kanji: Vec<RawForm>,
    #[serde(default)]
    kana: Vec<RawForm>,
    #[serde(default)]
    sense: Vec<RawSense>,
}

#[derive(Debug, Deserialize)]
struct RawForm {
    text: String,
    #[serde(default)]
    common: bool,
}

#[derive(Debug, Deserialize)]
struct RawSense {
    #[serde(default, rename = "partOfSpeech")]
    part_of_speech: Vec<String>,
    #[serde(default)]
    misc: Vec<String>,
    #[serde(default)]
    gloss: Vec<RawGloss>,
}

#[derive(Debug, Deserialize)]
struct RawGloss {
    text: String,
}

impl RawWord {
    /// Convert to an [`Entry`], or `None` for a word with no surface form at all.
    /// Such an entry would index under nothing and has no headword, so it is
    /// dropped at the parse boundary rather than carried as a degenerate value.
    pub(super) fn into_entry(self) -> Option<Entry> {
        if self.kanji.is_empty() && self.kana.is_empty() {
            return None;
        }
        let common = self.kanji.iter().any(|f| f.common) || self.kana.iter().any(|f| f.common);
        Some(Entry {
            kanji: self.kanji.into_iter().map(|f| f.text).collect(),
            kana: self.kana.into_iter().map(|f| f.text).collect(),
            senses: self
                .sense
                .into_iter()
                .map(|s| Sense {
                    part_of_speech: s.part_of_speech,
                    glosses: s.gloss.into_iter().map(|g| g.text).collect(),
                    misc: s.misc,
                })
                .collect(),
            common,
        })
    }
}
