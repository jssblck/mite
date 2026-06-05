//! Part-of-speech types.
//!
//! [`LinderaPos`] is the IPADIC major POS (品詞 大分類), parsed once from a
//! morpheme's first POS field. [`PosClass`] is a broad classifier for a single
//! JMdict POS abbreviation (`v5r`, `adj-i`, `prt`, …). Parsing POS into these at
//! the boundary keeps the grammatical matching out of the stringly-typed
//! `starts_with`/`==` checks that were previously scattered across the lookup
//! and overlay code.

/// IPADIC major part of speech, parsed from a Lindera morpheme's first POS
/// field. Unrecognized or empty fields become [`LinderaPos::Other`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinderaPos {
    /// 助詞 — particle.
    Particle,
    /// 助動詞 — auxiliary verb.
    AuxVerb,
    /// 動詞 — verb.
    Verb,
    /// 形容詞 — i-adjective.
    Adjective,
    /// 副詞 — adverb.
    Adverb,
    /// 連体詞 — prenominal adjectival.
    Adnominal,
    /// 接続詞 — conjunction.
    Conjunction,
    /// 感動詞 — interjection.
    Interjection,
    /// 接頭詞 — prefix.
    Prefix,
    /// 名詞 — noun.
    Noun,
    Other,
}

impl LinderaPos {
    /// Parse the IPADIC major-POS field.
    pub fn from_major(major: &str) -> Self {
        match major {
            "助詞" => Self::Particle,
            "助動詞" => Self::AuxVerb,
            "動詞" => Self::Verb,
            "形容詞" => Self::Adjective,
            "副詞" => Self::Adverb,
            "連体詞" => Self::Adnominal,
            "接続詞" => Self::Conjunction,
            "感動詞" => Self::Interjection,
            "接頭詞" => Self::Prefix,
            "名詞" => Self::Noun,
            _ => Self::Other,
        }
    }

    /// Whether a JMdict POS abbreviation grammatically agrees with this IPADIC
    /// major POS — used to surface the sense matching Lindera's reading of a
    /// homograph (は as the particle `prt`, not the noun 羽).
    pub fn agrees_with_jmdict(self, jmdict_pos: &str) -> bool {
        match self {
            Self::Particle => jmdict_pos == "prt",
            Self::AuxVerb => jmdict_pos.starts_with("aux") || jmdict_pos.starts_with("cop"),
            Self::Verb => jmdict_pos.starts_with('v'),
            Self::Adjective => jmdict_pos.starts_with("adj"),
            Self::Adverb => jmdict_pos.starts_with("adv"),
            Self::Adnominal => jmdict_pos == "adj-pn",
            Self::Conjunction => jmdict_pos == "conj",
            Self::Interjection => jmdict_pos == "int",
            Self::Prefix => jmdict_pos.starts_with("pref"),
            Self::Noun => jmdict_pos.starts_with('n') || jmdict_pos == "pn",
            Self::Other => false,
        }
    }
}

/// Broad grammatical class of a single JMdict POS tag, classified by the
/// meaningful prefixes JMdict uses (`v*` = verb, `adj*` = adjective, …).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PosClass {
    Particle,
    Noun,
    Verb,
    Adjective,
    Adverb,
    Expression,
    Auxiliary,
    Other,
}

impl PosClass {
    /// Classify one JMdict POS abbreviation. Prefix order matters: the more
    /// specific `aux`/`cop` and `adj`/`adv` prefixes are checked before the
    /// single-letter `v`/`n` fallbacks.
    pub fn of(jmdict_pos: &str) -> Self {
        if jmdict_pos == "prt" {
            Self::Particle
        } else if jmdict_pos == "exp" {
            Self::Expression
        } else if jmdict_pos.starts_with("aux") || jmdict_pos.starts_with("cop") {
            Self::Auxiliary
        } else if jmdict_pos.starts_with('v') {
            Self::Verb
        } else if jmdict_pos.starts_with("adj") {
            Self::Adjective
        } else if jmdict_pos.starts_with("adv") {
            Self::Adverb
        } else if jmdict_pos.starts_with('n') || jmdict_pos == "pn" {
            Self::Noun
        } else {
            Self::Other
        }
    }

    /// Whether fusing a span across a particle is justified: the fused entry is
    /// contentful (verb/adjective/adverb/expression) rather than a grammatical
    /// coincidence that merely happens to be a noun homograph.
    pub fn is_compound_worthy(self) -> bool {
        matches!(
            self,
            Self::Verb | Self::Adjective | Self::Adverb | Self::Expression
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ipadic_majors() {
        assert_eq!(LinderaPos::from_major("助詞"), LinderaPos::Particle);
        assert_eq!(LinderaPos::from_major("動詞"), LinderaPos::Verb);
        assert_eq!(LinderaPos::from_major(""), LinderaPos::Other);
        assert_eq!(LinderaPos::from_major("記号"), LinderaPos::Other);
    }

    #[test]
    fn lindera_major_agrees_with_jmdict_abbreviations() {
        assert!(LinderaPos::Particle.agrees_with_jmdict("prt"));
        assert!(!LinderaPos::Particle.agrees_with_jmdict("n"));
        assert!(LinderaPos::Verb.agrees_with_jmdict("v5r"));
        assert!(LinderaPos::AuxVerb.agrees_with_jmdict("aux-v"));
        assert!(LinderaPos::AuxVerb.agrees_with_jmdict("cop"));
        assert!(LinderaPos::Noun.agrees_with_jmdict("pn"));
        // 形容詞 covers adj-pn via prefix; 連体詞 matches it exactly.
        assert!(LinderaPos::Adjective.agrees_with_jmdict("adj-pn"));
        assert!(LinderaPos::Adnominal.agrees_with_jmdict("adj-pn"));
        assert!(!LinderaPos::Other.agrees_with_jmdict("n"));
    }

    #[test]
    fn classifies_jmdict_tags() {
        assert_eq!(PosClass::of("prt"), PosClass::Particle);
        assert_eq!(PosClass::of("exp"), PosClass::Expression);
        assert_eq!(PosClass::of("aux-v"), PosClass::Auxiliary);
        assert_eq!(PosClass::of("cop-da"), PosClass::Auxiliary);
        assert_eq!(PosClass::of("v1"), PosClass::Verb);
        assert_eq!(PosClass::of("adj-i"), PosClass::Adjective);
        assert_eq!(PosClass::of("adv-to"), PosClass::Adverb);
        assert_eq!(PosClass::of("n"), PosClass::Noun);
        assert_eq!(PosClass::of("pn"), PosClass::Noun);
        assert_eq!(PosClass::of("unc"), PosClass::Other);
    }

    #[test]
    fn only_contentful_classes_are_compound_worthy() {
        assert!(PosClass::of("v5r").is_compound_worthy());
        assert!(PosClass::of("adj-i").is_compound_worthy());
        assert!(PosClass::of("exp").is_compound_worthy());
        assert!(!PosClass::of("n").is_compound_worthy());
        assert!(!PosClass::of("prt").is_compound_worthy());
        assert!(!PosClass::of("aux-v").is_compound_worthy());
    }
}
