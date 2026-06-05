//! Offline Japanese dictionary lookup over OCR'd text.
//!
//! Segmentation and lemmatization are done by Lindera (embedded IPADIC, see
//! [`crate::morphology`]): each line is split into morphemes carrying a
//! dictionary (base) form, which is then looked up in the bundled JMdict lexicon
//! (scriptin/jmdict-simplified, CC BY-SA 4.0) for glosses. This replaced the
//! earlier hand-rolled longest-match segmentation + deinflection rule table.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use anyhow::{Context, Result};
use serde::Serialize;

use crate::frequency::FrequencyTable;
use crate::morphology::{Analyzer, Morpheme};
use crate::pos::{LinderaPos, PosClass};

mod raw;

use raw::RawWord;

/// Most consecutive morphemes a single token may span (二人, 朝ご飯, 手に入れる,
/// …). Bounds the lattice search.
const MAX_COMPOUND_MORPHEMES: usize = 6;

/// Cost added per token in the segmentation lattice. A small positive bias
/// toward fewer tokens; kept low so that splitting a rare false compound into
/// frequent function morphemes (してき -> し+て+き) still wins.
const TOKEN_PENALTY: f32 = 1.0;

/// A single dictionary sense: its parts of speech and English glosses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Sense {
    pub part_of_speech: Vec<String>,
    pub glosses: Vec<String>,
    /// JMdict `misc` tags (e.g. `arch`, `obs`, `rare`, `uk`). Used to demote
    /// archaic/obscure senses when ordering glosses for display.
    pub misc: Vec<String>,
}

/// One dictionary entry: written (kanji) forms, readings (kana), and senses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Entry {
    pub kanji: Vec<String>,
    pub kana: Vec<String>,
    pub senses: Vec<Sense>,
    /// True if any kanji/kana form is flagged "common" in JMdict. Used to bias
    /// segmentation away from rare homographs.
    pub common: bool,
}

impl Entry {
    /// The most representative headword: first kanji form, else first reading.
    pub fn headword(&self) -> &str {
        self.kanji
            .first()
            .or_else(|| self.kana.first())
            .map(String::as_str)
            .unwrap_or_default()
    }
}

/// An in-memory JMdict index keyed by every surface form (kanji and kana),
/// paired with a Lindera analyzer for segmentation + lemmatization.
pub struct Dictionary {
    entries: Vec<Entry>,
    by_form: HashMap<String, Vec<usize>>,
    analyzer: Analyzer,
    frequency: FrequencyTable,
}

impl Dictionary {
    fn with_parts(analyzer: Analyzer, frequency: FrequencyTable) -> Self {
        Self {
            entries: Vec::new(),
            by_form: HashMap::new(),
            analyzer,
            frequency,
        }
    }

    /// Stream-parse a jmdict-simplified JSON file. Each word entry sits on its
    /// own line, so we parse line by line instead of loading the whole file.
    /// Also loads the frequency table from a sibling `jpdb-freq/` directory (if
    /// present) to drive cost-based segmentation.
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let analyzer = Analyzer::new().context("failed to initialize morphological analyzer")?;
        let path = path.as_ref();
        let frequency = load_sibling_frequency(path);
        let file = File::open(path)
            .with_context(|| format!("failed to open lexicon {}", path.display()))?;
        let reader = BufReader::new(file);

        let mut dict = Dictionary::with_parts(analyzer, frequency);
        for line in reader.lines() {
            let line = line?;
            let trimmed = line.trim().trim_end_matches(',');
            if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
                continue;
            }
            // Header fragments and array brackets fail to parse and are skipped.
            let Ok(raw) = serde_json::from_str::<RawWord>(trimmed) else {
                continue;
            };
            if let Some(entry) = raw.into_entry() {
                dict.insert(entry);
            }
        }

        if dict.entries.is_empty() {
            anyhow::bail!(
                "no dictionary entries parsed from {}; is this a jmdict-simplified JSON file?",
                path.display()
            );
        }
        Ok(dict)
    }

    /// Build a dictionary directly from entries (used in tests). The frequency
    /// table is empty, so segmentation falls back to a fewest-tokens preference.
    pub fn from_entries(entries: Vec<Entry>) -> Self {
        let analyzer = Analyzer::new().expect("load embedded IPADIC dictionary");
        let mut dict = Dictionary::with_parts(analyzer, FrequencyTable::empty());
        for entry in entries {
            dict.insert(entry);
        }
        dict
    }

    fn insert(&mut self, entry: Entry) {
        let index = self.entries.len();
        for form in entry.kanji.iter().chain(entry.kana.iter()) {
            self.by_form.entry(form.clone()).or_default().push(index);
        }
        self.entries.push(entry);
    }

    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }

    /// Whether any JMdict entry is registered under this exact surface form.
    pub fn contains(&self, form: &str) -> bool {
        self.by_form.contains_key(form)
    }

    /// Whether any entry registered under this form is flagged common in JMdict.
    pub fn is_common(&self, form: &str) -> bool {
        self.entries_for(form)
            .is_some_and(|entries| entries.iter().any(|entry| entry.common))
    }

    /// Entries registered under an exact surface form, if any.
    fn entries_for(&self, form: &str) -> Option<Vec<&Entry>> {
        let indices = self.by_form.get(form)?;
        Some(indices.iter().map(|&i| &self.entries[i]).collect())
    }

    /// Segment a line into morphemes (Lindera), then choose the minimum-cost
    /// segmentation over a candidate lattice by dynamic programming (Viterbi) —
    /// the same idea Lindera/MeCab use at the morpheme layer, lifted to the
    /// JMdict-term layer. Each single morpheme is a candidate node, as is every
    /// adjacent span whose fused form is a JMdict entry; node cost is
    /// `ln(frequency rank)` (rarer = costlier) plus a small per-token penalty.
    ///
    /// This fuses real compounds (聖遺物, 必殺技 — rare-but-real, so cheaper as
    /// one node than as rarer-summed pieces) yet refuses grammatical
    /// coincidences (一掃して + き -> してき "史的"): the function morphemes are so
    /// frequent that splitting beats the rare false compound. No hand-tuned
    /// common/POS gate — frequency does the disambiguation.
    pub fn analyze_line(&self, line: &str) -> Vec<Token> {
        let morphemes = match self.analyzer.analyze(line) {
            Ok(morphemes) => morphemes,
            Err(error) => {
                tracing::warn!("morphological analysis failed for {line:?}: {error:#}");
                return Vec::new();
            }
        };
        if morphemes.is_empty() {
            return Vec::new();
        }

        let count = morphemes.len();
        let mut best_cost = vec![f32::INFINITY; count + 1];
        let mut back: Vec<Option<(usize, Token)>> = (0..=count).map(|_| None).collect();
        best_cost[0] = 0.0;

        for end in 1..=count {
            let earliest = end.saturating_sub(MAX_COMPOUND_MORPHEMES);
            for start in earliest..end {
                if best_cost[start].is_infinite() {
                    continue;
                }
                let Some((cost, token)) = self.node(&morphemes, start, end) else {
                    continue;
                };
                let total = best_cost[start] + cost;
                if total < best_cost[end] {
                    best_cost[end] = total;
                    back[end] = Some((start, token));
                }
            }
        }

        // Backtrack the minimum-cost path into tokens (reading order).
        let mut tokens = Vec::new();
        let mut end = count;
        while end > 0 {
            let (start, token) = back[end].take().expect("every position is reachable");
            tokens.push(token);
            end = start;
        }
        tokens.reverse();
        tokens
    }

    /// Cost and token for morpheme span `[start, end)`, or `None` when the span
    /// is not a valid lattice node. A single morpheme is always a node (resolved
    /// against JMdict, or an unknown token); a multi-morpheme span is a node only
    /// when its fused form (dictionary form first, then literal surface) is a
    /// JMdict entry that isn't a grammatical false-merge.
    fn node(&self, morphemes: &[Morpheme], start: usize, end: usize) -> Option<(f32, Token)> {
        let slice = &morphemes[start..end];
        let last = slice.last().expect("span is non-empty");
        let surface = span_surface(slice);

        match self.resolve_span(slice, &surface) {
            Some(resolution) => {
                if slice.len() > 1 && is_false_particle_merge(slice, &resolution.entries) {
                    return None;
                }
                let reasons = if resolution.matched_lemma && last.is_inflected() {
                    inflection_reasons(last)
                } else {
                    Vec::new()
                };
                let cost = self.frequency.cost(&resolution.form) + TOKEN_PENALTY;
                let token = Token {
                    surface,
                    dictionary_form: resolution.form,
                    reasons,
                    entries: ranked_entries(resolution.entries, last.major_pos()),
                };
                Some((cost, token))
            }
            // A lone unknown morpheme is still a node (its own surface), so the
            // lattice can always reach the end; a longer unresolved span is not.
            None if slice.len() == 1 => {
                let cost = self.frequency.cost(&last.base_form) + TOKEN_PENALTY;
                Some((cost, unknown_token(last)))
            }
            None => None,
        }
    }

    /// Resolve a span against JMdict, preferring the deinflected lemma form over
    /// the literal surface. `surface` is the precomputed literal surface.
    fn resolve_span(&self, slice: &[Morpheme], surface: &str) -> Option<Resolution<'_>> {
        let lemma = span_lemma(slice);
        if let Some(entries) = self.entries_for(&lemma) {
            return Some(Resolution {
                form: lemma,
                entries,
                matched_lemma: true,
            });
        }
        let entries = self.entries_for(surface)?;
        Some(Resolution {
            form: surface.to_string(),
            entries,
            matched_lemma: false,
        })
    }
}

/// A span resolved against JMdict: the headword form that matched, its entries,
/// and whether the match was on the deinflected lemma (vs. the literal surface).
struct Resolution<'a> {
    form: String,
    entries: Vec<&'a Entry>,
    matched_lemma: bool,
}

/// Concatenated literal surfaces of a span.
fn span_surface(slice: &[Morpheme]) -> String {
    slice.iter().map(|m| m.surface.as_str()).collect()
}

/// A span's fused form with only the last morpheme deinflected to its base form
/// (e.g. 食べ + ました → 食べる, but the leading morphemes stay as surface).
fn span_lemma(slice: &[Morpheme]) -> String {
    let (last, head) = slice.split_last().expect("span is non-empty");
    head.iter()
        .map(|m| m.surface.as_str())
        .chain(std::iter::once(last.base_form.as_str()))
        .collect()
}

/// Whether a multi-morpheme span is a grammatical false-merge: it swallows a
/// particle (助詞) yet the fused entry is not contentful. A purely nominal match
/// (いて = 射手, n) is a coincidence of the て-form + 居る pattern, so we reject it
/// and let the lattice split い + て instead; real compounds (手に入れる = exp,v1;
/// 一緒に = adv) survive because they carry a verb/adjective/adverb/expression sense.
fn is_false_particle_merge(slice: &[Morpheme], entries: &[&Entry]) -> bool {
    slice.iter().any(is_particle_morpheme)
        && !entries.iter().any(|entry| has_compound_worthy_pos(entry))
}

/// Clone entries, ordering the sense that agrees with Lindera's major POS first
/// so grammatical morphemes lead with their particle/auxiliary sense (は 助詞 →
/// topic marker; う 助動詞 → volitional) rather than a frequent noun homograph.
fn ranked_entries(entries: Vec<&Entry>, major: LinderaPos) -> Vec<Entry> {
    let mut entries: Vec<Entry> = entries.into_iter().cloned().collect();
    entries.sort_by_key(|entry| !entry_matches_lindera_pos(entry, major));
    entries
}

/// Load the frequency table from a `jpdb-freq/` directory beside the lexicon.
/// Missing/unreadable ⇒ an empty table (segmentation still works, but without
/// frequency disambiguation it degrades to a fewest-tokens preference).
fn load_sibling_frequency(lexicon: &Path) -> FrequencyTable {
    let Some(dir) = lexicon.parent().map(|parent| parent.join("jpdb-freq")) else {
        return FrequencyTable::empty();
    };
    if !dir.is_dir() {
        tracing::warn!(
            "no frequency dictionary at {}; segmentation quality reduced (install the JPDB freq dict)",
            dir.display()
        );
        return FrequencyTable::empty();
    }
    match FrequencyTable::load(&dir) {
        Ok(table) => {
            tracing::info!(
                "loaded {} frequency entries from {}",
                table.len(),
                dir.display()
            );
            table
        }
        Err(error) => {
            tracing::warn!(
                "failed to load frequency dictionary from {}: {error:#}",
                dir.display()
            );
            FrequencyTable::empty()
        }
    }
}

/// A token for a morpheme with no JMdict entry: reports the lemma as the
/// dictionary form but carries no entries (so `is_known()` is false).
fn unknown_token(morpheme: &Morpheme) -> Token {
    Token {
        surface: morpheme.surface.clone(),
        dictionary_form: morpheme.base_form.clone(),
        reasons: Vec::new(),
        entries: Vec::new(),
    }
}

/// Whether a morpheme is an IPADIC particle (助詞). Particles are word
/// boundaries, so they should not be absorbed into a content-word fusion.
fn is_particle_morpheme(morpheme: &Morpheme) -> bool {
    morpheme.major_pos() == LinderaPos::Particle
}

/// Whether a JMdict entry is "contentful" enough to justify fusing a span that
/// contains a particle: a verb, adjective, adverb, or set expression. Pure
/// nouns (and noun-only homographs) do not qualify.
fn has_compound_worthy_pos(entry: &Entry) -> bool {
    entry
        .senses
        .iter()
        .flat_map(|sense| sense.part_of_speech.iter())
        .any(|pos| PosClass::of(pos).is_compound_worthy())
}

/// Whether a JMdict entry has any sense whose part of speech agrees with a
/// Lindera (IPADIC) major part of speech. Used to surface the grammatically
/// correct sense of a homograph (は as the particle 助詞, not the noun 羽).
fn entry_matches_lindera_pos(entry: &Entry, major: LinderaPos) -> bool {
    entry
        .senses
        .iter()
        .flat_map(|sense| sense.part_of_speech.iter())
        .any(|pos| major.agrees_with_jmdict(pos))
}

/// A short note describing how an inflected surface relates to its base form,
/// derived from the IPADIC conjugation-form feature (e.g. 連用形). Empty for
/// uninflected morphemes.
fn inflection_reasons(morpheme: &Morpheme) -> Vec<String> {
    if morpheme.is_inflected() {
        morpheme.conjugation_form.clone().into_iter().collect()
    } else {
        Vec::new()
    }
}

/// A segmented token and its dictionary resolution (if any).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Token {
    /// The exact recognized substring.
    pub surface: String,
    /// The resolved dictionary headword (== surface for an exact match).
    pub dictionary_form: String,
    /// Deinflection reasons applied, outermost first (empty for exact matches).
    pub reasons: Vec<String>,
    /// Matching dictionary entries (empty for unknown tokens).
    pub entries: Vec<Entry>,
}

impl Token {
    pub fn is_known(&self) -> bool {
        !self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(kanji: &[&str], kana: &[&str], pos: &str, glosses: &[&str]) -> Entry {
        entry_with_common(kanji, kana, pos, glosses, true)
    }

    fn entry_with_common(
        kanji: &[&str],
        kana: &[&str],
        pos: &str,
        glosses: &[&str],
        common: bool,
    ) -> Entry {
        Entry {
            kanji: kanji.iter().map(|s| s.to_string()).collect(),
            kana: kana.iter().map(|s| s.to_string()).collect(),
            senses: vec![Sense {
                part_of_speech: vec![pos.to_string()],
                glosses: glosses.iter().map(|s| s.to_string()).collect(),
                misc: Vec::new(),
            }],
            common,
        }
    }

    fn sample_dict() -> Dictionary {
        Dictionary::from_entries(vec![
            entry(&["食べる"], &["たべる"], "v1", &["to eat"]),
            entry(&["飲む"], &["のむ"], "v5m", &["to drink"]),
            entry(&["買う"], &["かう"], "v5u", &["to buy"]),
            entry(&["高い"], &["たかい"], "adj-i", &["high", "expensive"]),
            entry(&["水"], &["みず"], "n", &["water"]),
        ])
    }

    /// Dictionary forms of the known tokens for a line, in order.
    fn known_forms(dict: &Dictionary, line: &str) -> Vec<String> {
        dict.analyze_line(line)
            .into_iter()
            .filter(Token::is_known)
            .map(|token| token.dictionary_form)
            .collect()
    }

    #[test]
    fn resolves_uninflected_noun() {
        let dict = sample_dict();
        let token = dict
            .analyze_line("水")
            .into_iter()
            .find(Token::is_known)
            .expect("known token");
        assert_eq!(token.surface, "水");
        assert_eq!(token.dictionary_form, "水");
        assert!(token.reasons.is_empty());
    }

    #[test]
    fn lemmatizes_inflected_verbs_and_adjectives() {
        // Lindera segments + lemmatizes; the lemma is looked up in JMdict.
        let dict = sample_dict();
        assert!(known_forms(&dict, "食べた").contains(&"食べる".to_string()));
        assert!(known_forms(&dict, "食べました").contains(&"食べる".to_string()));
        assert!(known_forms(&dict, "飲みます").contains(&"飲む".to_string()));
        assert!(known_forms(&dict, "買った").contains(&"買う".to_string()));
        assert!(known_forms(&dict, "高かった").contains(&"高い".to_string()));
    }

    #[test]
    fn inflected_form_carries_a_reason_note() {
        let dict = sample_dict();
        let verb = dict
            .analyze_line("食べた")
            .into_iter()
            .find(|token| token.dictionary_form == "食べる")
            .expect("verb token");
        assert_ne!(verb.surface, verb.dictionary_form);
        // The conjugation-form note (IPADIC 活用形) is surfaced as a reason.
        assert!(!verb.reasons.is_empty());
    }

    #[test]
    fn resolves_suru_after_noun() {
        let dict = Dictionary::from_entries(vec![
            entry(&["勉強"], &["べんきょう"], "n", &["study"]),
            // include kanji + kana orthographies of suru so the lemma resolves
            // regardless of which IPADIC writes for 原形.
            entry(&["為る", "する"], &["する"], "vs-i", &["to do"]),
        ]);
        let forms = known_forms(&dict, "勉強します");
        assert!(forms.contains(&"勉強".to_string()), "forms: {forms:?}");
        assert!(
            forms.iter().any(|f| f == "する" || f == "為る"),
            "forms: {forms:?}"
        );
    }

    #[test]
    fn keeps_surface_spelling_over_homograph_headword() {
        // 本 is registered under both a "book" entry and an "origin" (元) entry.
        // The resolved form must report 本, not the other entry's headword 元.
        let dict = Dictionary::from_entries(vec![
            entry(&["元", "本"], &["もと"], "n", &["origin"]),
            entry(&["本"], &["ほん"], "n", &["book"]),
        ]);
        let token = dict
            .analyze_line("本")
            .into_iter()
            .find(Token::is_known)
            .expect("known token");
        assert_eq!(token.dictionary_form, "本");
        assert!(token.reasons.is_empty());
        assert_eq!(token.entries.len(), 2);
    }

    #[test]
    fn particle_prefers_particle_sense_over_noun_homograph() {
        // は has a frequent noun homograph (羽 "feather") but Lindera tags it
        // 助詞; the resolved token must surface the particle sense first so the
        // popup/category show "topic marker", not "feather".
        let dict = Dictionary::from_entries(vec![
            entry(&["今日"], &["きょう"], "n", &["today"]),
            entry(&["羽", "羽根"], &["はね"], "n", &["feather"]),
            entry_with_common(&[], &["は"], "prt", &["indicates sentence topic"], true),
        ]);
        let token = dict
            .analyze_line("今日は")
            .into_iter()
            .find(|token| token.surface == "は")
            .expect("は token");
        let pos = &token
            .entries
            .first()
            .expect("entry")
            .senses
            .first()
            .expect("sense")
            .part_of_speech;
        assert_eq!(pos, &vec!["prt".to_string()]);
    }

    #[test]
    fn auxiliary_prefers_aux_sense_over_noun_homograph() {
        // The volitional う (助動詞) shares a surface with the noun 鵜 (cormorant).
        let dict = Dictionary::from_entries(vec![
            entry(&["帰る"], &["かえる"], "v5r", &["to return"]),
            entry(&["鵜"], &["う"], "n", &["cormorant"]),
            entry_with_common(
                &[],
                &["う"],
                "aux-v",
                &["indicates speaker's volition"],
                true,
            ),
        ]);
        let token = dict
            .analyze_line("帰ろう")
            .into_iter()
            .find(|token| token.surface == "う")
            .expect("う token");
        let pos = &token
            .entries
            .first()
            .expect("entry")
            .senses
            .first()
            .expect("sense")
            .part_of_speech;
        assert_eq!(pos, &vec!["aux-v".to_string()]);
    }

    #[test]
    fn particle_is_not_fused_into_noun_homograph() {
        // 待っていて segments as 待っ / て / い / て; the trailing い + て must not
        // fuse into the noun 射手 (いて, "archer"), a grammatical false-merge.
        let dict = Dictionary::from_entries(vec![
            entry(&["待つ"], &["まつ"], "v5t", &["to wait"]),
            entry(&["居る"], &["いる"], "v1", &["to be (animate)"]),
            entry(&["射手"], &["いて"], "n", &["archer"]),
        ]);
        let tokens = dict.analyze_line("待っていて");
        assert!(
            tokens.iter().all(|token| token.dictionary_form != "射手"),
            "いて wrongly fused into 射手: {tokens:?}"
        );
    }

    #[test]
    fn contentful_compound_with_particle_still_fuses() {
        // The guard must not block legitimate particle-bearing compounds: 一緒に
        // (一緒 + に, adv) and 手に入れる (exp,v1) should still fuse to one token.
        let dict = Dictionary::from_entries(vec![
            entry(&["一緒"], &["いっしょ"], "n", &["together"]),
            entry(&["一緒に"], &["いっしょに"], "adv", &["together (with)"]),
            entry(&["手に入れる"], &["てにいれる"], "exp", &["to obtain"]),
        ]);
        assert!(
            known_forms(&dict, "一緒に").contains(&"一緒に".to_string()),
            "一緒に should fuse"
        );
        assert!(
            known_forms(&dict, "手に入れる").contains(&"手に入れる".to_string()),
            "手に入れる should fuse"
        );
    }

    #[test]
    fn unknown_tokens_have_no_entries() {
        let dict = sample_dict();
        let tokens = dict.analyze_line("食べるXYZ");
        assert!(
            tokens
                .iter()
                .any(|token| token.is_known() && token.dictionary_form == "食べる")
        );
        assert!(tokens.iter().any(|token| !token.is_known()));
    }
}
