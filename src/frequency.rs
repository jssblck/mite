//! Word-frequency table for cost-based segmentation.
//!
//! Loads a Yomitan rank-based frequency dictionary (JPDB: scraped from a corpus
//! of anime/drama/light-novel/visual-novel/game text, so it ranks domain
//! vocabulary far better than a general corpus would). Each term maps to a rank
//! (1 = most frequent); the segmentation lattice uses `cost = ln(rank)` so rarer
//! words cost more and a real compound beats splitting into rarer pieces.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

/// Cost assigned to a form absent from the table — treated as rarer than the
/// rarest ranked word (the list tops out around rank ~5×10^5, ln ≈ 13.1).
const UNKNOWN_COST: f32 = 13.5;

/// Yomitan frequency-bank file naming and the entry `mode` that marks a
/// frequency record (vs. pitch-accent etc.).
const BANK_FILE_PREFIX: &str = "term_meta_bank_";
const BANK_FILE_SUFFIX: &str = ".json";
const FREQ_MODE: &str = "freq";

/// Maps a surface/dictionary form to its frequency rank (1 = most frequent).
#[derive(Debug, Default)]
pub struct FrequencyTable {
    ranks: HashMap<String, u32>,
}

impl FrequencyTable {
    /// An empty table: every form costs [`UNKNOWN_COST`]. Used in tests and when
    /// no frequency dictionary is installed.
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.ranks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ranks.is_empty()
    }

    /// Load every `term_meta_bank_*.json` from an extracted Yomitan frequency
    /// dictionary directory, keeping the best (lowest) rank seen per form.
    pub fn load(dir: &Path) -> Result<Self> {
        let mut banks = fs::read_dir(dir)
            .with_context(|| format!("failed to read frequency dir {}", dir.display()))?
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| {
                        name.starts_with(BANK_FILE_PREFIX) && name.ends_with(BANK_FILE_SUFFIX)
                    })
            })
            .collect::<Vec<_>>();
        banks.sort();

        let mut ranks: HashMap<String, u32> = HashMap::new();
        for bank in banks {
            let text = fs::read_to_string(&bank)
                .with_context(|| format!("failed to read {}", bank.display()))?;
            let entries: Vec<RawEntry> = serde_json::from_str(&text)
                .with_context(|| format!("failed to parse {}", bank.display()))?;
            for entry in entries {
                if entry.mode != FREQ_MODE {
                    continue;
                }
                if let Some(rank) = entry.data.rank() {
                    ranks
                        .entry(entry.term)
                        .and_modify(|existing| *existing = (*existing).min(rank))
                        .or_insert(rank);
                }
            }
        }

        if ranks.is_empty() {
            bail!(
                "no frequency entries parsed from {}; is this a Yomitan freq dictionary?",
                dir.display()
            );
        }
        Ok(Self { ranks })
    }

    /// Segmentation cost of a form: `ln(rank)`, or [`UNKNOWN_COST`] if unranked.
    pub fn cost(&self, form: &str) -> f32 {
        match self.ranks.get(form) {
            Some(&rank) => (rank.max(1) as f32).ln(),
            None => UNKNOWN_COST,
        }
    }

    /// The frequency rank of a form (1 = most frequent), if known.
    pub fn rank(&self, form: &str) -> Option<u32> {
        self.ranks.get(form).copied()
    }
}

/// One `["term", "freq", <data>]` entry in a term_meta_bank.
#[derive(Deserialize)]
struct RawEntry {
    term: String,
    mode: String,
    data: RawFreq,
}

/// The frequency payload: either `{value, displayValue}` directly, or nested
/// under a reading as `{reading, frequency: {value, ...}}`.
#[derive(Deserialize)]
#[serde(untagged)]
enum RawFreq {
    Nested { frequency: RawRank },
    Flat(RawRank),
}

impl RawFreq {
    fn rank(&self) -> Option<u32> {
        let rank = match self {
            RawFreq::Nested { frequency } => frequency,
            RawFreq::Flat(rank) => rank,
        };
        rank.value
            .and_then(|value| u32::try_from(value).ok())
            .filter(|&value| value > 0)
    }
}

#[derive(Deserialize)]
struct RawRank {
    #[serde(default)]
    value: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_flat_and_nested_entries_keeping_best_rank() {
        let json = r#"[
            ["の","freq",{"value":1,"displayValue":"1㋕"}],
            ["為る","freq",{"reading":"する","frequency":{"value":12,"displayValue":"12㋕"}}],
            ["欠落","freq",{"displayValue":"❌"}]
        ]"#;
        let dir = std::env::temp_dir().join(format!(
            "mite-freq-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("term_meta_bank_1.json"), json).unwrap();

        let table = FrequencyTable::load(&dir).unwrap();
        assert_eq!(table.rank("の"), Some(1));
        assert_eq!(table.rank("為る"), Some(12));
        // ❌ (no usable value) is skipped.
        assert_eq!(table.rank("欠落"), None);
        // ln(rank): の (rank 1) costs 0; an unknown form costs UNKNOWN_COST.
        assert!(table.cost("の") < 0.001);
        assert!(table.cost("欠落") > 13.0);

        fs::remove_dir_all(dir).unwrap();
    }
}
