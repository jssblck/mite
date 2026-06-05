use crate::dictionary::{Sense, Token};

/// JMdict `misc` tags that mark a sense as not the meaning a reader of modern
/// text usually wants; such senses sink below ordinary ones when ordering.
const DEMOTE_MISC: &[&str] = &["arch", "obs", "rare", "dated"];

/// Contextual signal for ordering an entry's senses, derived from the words
/// surrounding the hovered token (see [`transitivity_hint`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SenseHint {
    /// Transitivity suggested by nearby particles: `Some(true)` when an
    /// object-marking を precedes a verb (prefer transitive senses), `None`
    /// when there is no signal. We never infer intransitive from the absence
    /// of を (too noisy), so this is `Some(false)` only if set explicitly.
    pub transitive: Option<bool>,
}

/// Suggest transitivity for a verb token from the particles preceding it in the
/// same clause: an object-marking を implies a transitive reading. Scans back
/// to the previous clause boundary (sentence punctuation). High-precision: only
/// を sets the hint; absence of を leaves it unknown.
pub fn transitivity_hint(tokens: &[Token], index: usize) -> SenseHint {
    for prev in tokens[..index].iter().rev() {
        let surface = prev.surface.as_str();
        if surface == "を" {
            return SenseHint {
                transitive: Some(true),
            };
        }
        if surface
            .chars()
            .all(|c| matches!(c, '。' | '、' | '！' | '？' | '．' | '，' | '!' | '?'))
            && !surface.is_empty()
        {
            break;
        }
    }
    SenseHint::default()
}

/// Order an entry's senses for display as a stable reordering (JMdict's curated
/// order is preserved within each tier): archaic/obscure senses sink, and when a
/// transitivity hint is present, senses matching it rise above mismatched ones.
/// Returns sense indices in display order. Demotion dominates transitivity, so a
/// rare-but-transitive sense never leapfrogs a common one.
pub(super) fn ordered_sense_indices(senses: &[Sense], hint: SenseHint) -> Vec<usize> {
    let mut order: Vec<usize> = (0..senses.len()).collect();
    order.sort_by_key(|&i| {
        let sense = &senses[i];
        let demoted = sense
            .misc
            .iter()
            .any(|tag| DEMOTE_MISC.contains(&tag.as_str()));
        let has = |needle: &str| sense.part_of_speech.iter().any(|pos| pos == needle);
        let transitivity_penalty = match hint.transitive {
            Some(true) => !has("vt") && has("vi"),
            Some(false) => !has("vi") && has("vt"),
            None => false,
        };
        (demoted, transitivity_penalty, i)
    });
    order
}
