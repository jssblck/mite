//! Recursive Japanese deinflection for dictionary lookup candidates.
//!
//! Lindera gives us morpheme boundaries and per-morpheme base forms. This layer
//! solves the adjacent hover-dictionary problem: given a surface span, recursively
//! peel inflection suffixes into candidate JMdict headwords, then let the caller
//! validate those candidates against real dictionary entries and POS tags.

use std::collections::{HashSet, VecDeque};

use super::Entry;

const MAX_DEPTH: usize = 8;
const MAX_CANDIDATES: usize = 96;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct WordType(u32);

impl WordType {
    const ICHIDAN: Self = Self(1 << 0);
    const GODAN: Self = Self(1 << 1);
    const I_ADJ: Self = Self(1 << 2);
    const KURU: Self = Self(1 << 3);
    const SURU: Self = Self(1 << 4);
    const NOUN_VS: Self = Self(1 << 5);
    const VERB: Self =
        Self(Self::ICHIDAN.0 | Self::GODAN.0 | Self::KURU.0 | Self::SURU.0 | Self::NOUN_VS.0);
    const INFLECTABLE: Self = Self(Self::VERB.0 | Self::I_ADJ.0);
    const MASU_STEM: Self = Self(1 << 6);
    const INITIAL: Self = Self(1 << 7);

    const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    const fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Candidate {
    pub form: String,
    pub reasons: Vec<String>,
    pub word_type: WordType,
}

#[derive(Debug, Clone, Copy)]
struct Rule {
    from: &'static str,
    to: &'static str,
    from_type: WordType,
    to_type: WordType,
    reasons: &'static [&'static str],
}

const RULES: &[Rule] = &[
    // Polite forms.
    rule(
        "ませんでした",
        "",
        WordType::INITIAL,
        WordType::MASU_STEM,
        &["丁寧", "否定", "過去"],
    ),
    rule(
        "ません",
        "",
        WordType::INITIAL,
        WordType::MASU_STEM,
        &["丁寧", "否定"],
    ),
    rule(
        "ましょう",
        "",
        WordType::INITIAL,
        WordType::MASU_STEM,
        &["丁寧", "意志"],
    ),
    rule(
        "ました",
        "",
        WordType::INITIAL,
        WordType::MASU_STEM,
        &["丁寧", "過去"],
    ),
    rule(
        "ます",
        "",
        WordType::INITIAL,
        WordType::MASU_STEM,
        &["丁寧"],
    ),
    // Ichidan stems and adjective chains.
    rule(
        "たくなかった",
        "たい",
        WordType::INITIAL,
        WordType::I_ADJ,
        &["否定", "過去"],
    ),
    rule(
        "たくない",
        "たい",
        WordType::INITIAL,
        WordType::I_ADJ,
        &["否定"],
    ),
    rule(
        "たかった",
        "たい",
        WordType::INITIAL,
        WordType::I_ADJ,
        &["過去"],
    ),
    rule(
        "くなかった",
        "い",
        WordType::INITIAL,
        WordType::I_ADJ,
        &["否定", "過去"],
    ),
    rule("くない", "い", WordType::I_ADJ, WordType::I_ADJ, &["否定"]),
    rule(
        "かった",
        "い",
        WordType::INITIAL,
        WordType::I_ADJ,
        &["過去"],
    ),
    rule(
        "ければ",
        "い",
        WordType::INITIAL,
        WordType::I_ADJ,
        &["仮定"],
    ),
    rule("くて", "い", WordType::INITIAL, WordType::I_ADJ, &["て形"]),
    rule("たい", "る", WordType::I_ADJ, WordType::ICHIDAN, &["願望"]),
    rule("ない", "る", WordType::I_ADJ, WordType::ICHIDAN, &["否定"]),
    rule("た", "る", WordType::INITIAL, WordType::ICHIDAN, &["過去"]),
    rule(
        "れば",
        "る",
        WordType::INITIAL,
        WordType::ICHIDAN,
        &["仮定"],
    ),
    rule("ろ", "る", WordType::INITIAL, WordType::ICHIDAN, &["命令"]),
    // Suru/kuru irregulars.
    rule(
        "こなかった",
        "くる",
        WordType::INITIAL,
        WordType::KURU,
        &["否定", "過去"],
    ),
    rule(
        "来なかった",
        "来る",
        WordType::INITIAL,
        WordType::KURU,
        &["否定", "過去"],
    ),
    rule("こない", "くる", WordType::I_ADJ, WordType::KURU, &["否定"]),
    rule("来ない", "来る", WordType::I_ADJ, WordType::KURU, &["否定"]),
    rule(
        "きました",
        "くる",
        WordType::INITIAL,
        WordType::KURU,
        &["丁寧", "過去"],
    ),
    rule(
        "来ました",
        "来る",
        WordType::INITIAL,
        WordType::KURU,
        &["丁寧", "過去"],
    ),
    rule(
        "きます",
        "くる",
        WordType::INITIAL,
        WordType::KURU,
        &["丁寧"],
    ),
    rule(
        "来ます",
        "来る",
        WordType::INITIAL,
        WordType::KURU,
        &["丁寧"],
    ),
    rule("きた", "くる", WordType::INITIAL, WordType::KURU, &["過去"]),
    rule("来た", "来る", WordType::INITIAL, WordType::KURU, &["過去"]),
    rule("きて", "くる", WordType::INITIAL, WordType::KURU, &["て形"]),
    rule("来て", "来る", WordType::INITIAL, WordType::KURU, &["て形"]),
    rule(
        "こられる",
        "くる",
        WordType::ICHIDAN,
        WordType::KURU,
        &["可能", "受身"],
    ),
    rule(
        "来られる",
        "来る",
        WordType::ICHIDAN,
        WordType::KURU,
        &["可能", "受身"],
    ),
    rule(
        "こさせる",
        "くる",
        WordType::ICHIDAN,
        WordType::KURU,
        &["使役"],
    ),
    rule(
        "来させる",
        "来る",
        WordType::ICHIDAN,
        WordType::KURU,
        &["使役"],
    ),
    rule(
        "した",
        "する",
        WordType::INITIAL,
        WordType::SURU.union(WordType::NOUN_VS),
        &["過去"],
    ),
    rule(
        "して",
        "する",
        WordType::INITIAL,
        WordType::SURU.union(WordType::NOUN_VS),
        &["連用形"],
    ),
    rule(
        "し",
        "する",
        WordType::INITIAL,
        WordType::SURU.union(WordType::NOUN_VS),
        &["連用形"],
    ),
    rule(
        "しない",
        "する",
        WordType::I_ADJ,
        WordType::SURU.union(WordType::NOUN_VS),
        &["否定"],
    ),
    rule(
        "すれば",
        "する",
        WordType::INITIAL,
        WordType::SURU.union(WordType::NOUN_VS),
        &["仮定"],
    ),
    rule(
        "します",
        "する",
        WordType::INITIAL,
        WordType::SURU.union(WordType::NOUN_VS),
        &["丁寧"],
    ),
    rule(
        "しました",
        "する",
        WordType::INITIAL,
        WordType::SURU.union(WordType::NOUN_VS),
        &["丁寧", "過去"],
    ),
    rule(
        "しません",
        "する",
        WordType::INITIAL,
        WordType::SURU.union(WordType::NOUN_VS),
        &["丁寧", "否定"],
    ),
    rule(
        "しませんでした",
        "する",
        WordType::INITIAL,
        WordType::SURU.union(WordType::NOUN_VS),
        &["丁寧", "否定", "過去"],
    ),
    rule(
        "される",
        "する",
        WordType::ICHIDAN,
        WordType::SURU.union(WordType::NOUN_VS),
        &["受身"],
    ),
    rule(
        "させる",
        "する",
        WordType::ICHIDAN,
        WordType::SURU.union(WordType::NOUN_VS),
        &["使役"],
    ),
    rule(
        "られる",
        "る",
        WordType::ICHIDAN,
        WordType::ICHIDAN,
        &["可能", "受身"],
    ),
    rule(
        "させる",
        "る",
        WordType::ICHIDAN,
        WordType::ICHIDAN,
        &["使役"],
    ),
    // Contracted te-auxiliary patterns. We avoid an ichidan bare `て` -> `る`
    // rule so `見ている` can still become 見る + auxiliary いる.
    rule(
        "ってた",
        "う",
        WordType::INITIAL,
        WordType::GODAN,
        &["継続", "過去"],
    ),
    rule(
        "ってた",
        "つ",
        WordType::INITIAL,
        WordType::GODAN,
        &["継続", "過去"],
    ),
    rule(
        "ってた",
        "る",
        WordType::INITIAL,
        WordType::GODAN,
        &["継続", "過去"],
    ),
    rule(
        "いてた",
        "く",
        WordType::INITIAL,
        WordType::GODAN,
        &["継続", "過去"],
    ),
    rule(
        "いでた",
        "ぐ",
        WordType::INITIAL,
        WordType::GODAN,
        &["継続", "過去"],
    ),
    rule(
        "してた",
        "す",
        WordType::INITIAL,
        WordType::GODAN,
        &["継続", "過去"],
    ),
    rule(
        "んでた",
        "む",
        WordType::INITIAL,
        WordType::GODAN,
        &["継続", "過去"],
    ),
    rule(
        "んでた",
        "ぶ",
        WordType::INITIAL,
        WordType::GODAN,
        &["継続", "過去"],
    ),
    rule(
        "んでた",
        "ぬ",
        WordType::INITIAL,
        WordType::GODAN,
        &["継続", "過去"],
    ),
    rule(
        "っちゃった",
        "う",
        WordType::INITIAL,
        WordType::GODAN,
        &["ちゃう", "過去"],
    ),
    rule(
        "っちゃった",
        "つ",
        WordType::INITIAL,
        WordType::GODAN,
        &["ちゃう", "過去"],
    ),
    rule(
        "っちゃった",
        "る",
        WordType::INITIAL,
        WordType::GODAN,
        &["ちゃう", "過去"],
    ),
    rule(
        "いちゃった",
        "く",
        WordType::INITIAL,
        WordType::GODAN,
        &["ちゃう", "過去"],
    ),
    rule(
        "いじゃった",
        "ぐ",
        WordType::INITIAL,
        WordType::GODAN,
        &["じゃう", "過去"],
    ),
    rule(
        "しちゃった",
        "す",
        WordType::INITIAL,
        WordType::GODAN,
        &["ちゃう", "過去"],
    ),
    rule(
        "んじゃった",
        "む",
        WordType::INITIAL,
        WordType::GODAN,
        &["じゃう", "過去"],
    ),
    rule(
        "んじゃった",
        "ぶ",
        WordType::INITIAL,
        WordType::GODAN,
        &["じゃう", "過去"],
    ),
    rule(
        "んじゃった",
        "ぬ",
        WordType::INITIAL,
        WordType::GODAN,
        &["じゃう", "過去"],
    ),
];

const MASU_STEM_RULES: &[Rule] = &[
    rule(
        "",
        "る",
        WordType::MASU_STEM,
        WordType::ICHIDAN,
        &["連用形"],
    ),
    rule(
        "い",
        "う",
        WordType::MASU_STEM,
        WordType::GODAN,
        &["連用形"],
    ),
    rule(
        "き",
        "く",
        WordType::MASU_STEM,
        WordType::GODAN,
        &["連用形"],
    ),
    rule(
        "ぎ",
        "ぐ",
        WordType::MASU_STEM,
        WordType::GODAN,
        &["連用形"],
    ),
    rule(
        "し",
        "す",
        WordType::MASU_STEM,
        WordType::GODAN,
        &["連用形"],
    ),
    rule(
        "ち",
        "つ",
        WordType::MASU_STEM,
        WordType::GODAN,
        &["連用形"],
    ),
    rule(
        "に",
        "ぬ",
        WordType::MASU_STEM,
        WordType::GODAN,
        &["連用形"],
    ),
    rule(
        "び",
        "ぶ",
        WordType::MASU_STEM,
        WordType::GODAN,
        &["連用形"],
    ),
    rule(
        "み",
        "む",
        WordType::MASU_STEM,
        WordType::GODAN,
        &["連用形"],
    ),
    rule(
        "り",
        "る",
        WordType::MASU_STEM,
        WordType::GODAN,
        &["連用形"],
    ),
];

const GODAN_RULES: &[Rule] = &[
    // Negative / irrealis.
    rule("わない", "う", WordType::I_ADJ, WordType::GODAN, &["否定"]),
    rule("かない", "く", WordType::I_ADJ, WordType::GODAN, &["否定"]),
    rule("がない", "ぐ", WordType::I_ADJ, WordType::GODAN, &["否定"]),
    rule("さない", "す", WordType::I_ADJ, WordType::GODAN, &["否定"]),
    rule("たない", "つ", WordType::I_ADJ, WordType::GODAN, &["否定"]),
    rule("なない", "ぬ", WordType::I_ADJ, WordType::GODAN, &["否定"]),
    rule("ばない", "ぶ", WordType::I_ADJ, WordType::GODAN, &["否定"]),
    rule("まない", "む", WordType::I_ADJ, WordType::GODAN, &["否定"]),
    rule("らない", "る", WordType::I_ADJ, WordType::GODAN, &["否定"]),
    // Past and godan te-form.
    rule("った", "う", WordType::INITIAL, WordType::GODAN, &["過去"]),
    rule("った", "つ", WordType::INITIAL, WordType::GODAN, &["過去"]),
    rule("った", "る", WordType::INITIAL, WordType::GODAN, &["過去"]),
    rule("いた", "く", WordType::INITIAL, WordType::GODAN, &["過去"]),
    rule("いだ", "ぐ", WordType::INITIAL, WordType::GODAN, &["過去"]),
    rule("した", "す", WordType::INITIAL, WordType::GODAN, &["過去"]),
    rule("んだ", "む", WordType::INITIAL, WordType::GODAN, &["過去"]),
    rule("んだ", "ぶ", WordType::INITIAL, WordType::GODAN, &["過去"]),
    rule("んだ", "ぬ", WordType::INITIAL, WordType::GODAN, &["過去"]),
    rule(
        "行った",
        "行く",
        WordType::INITIAL,
        WordType::GODAN,
        &["過去"],
    ),
    rule(
        "いった",
        "いく",
        WordType::INITIAL,
        WordType::GODAN,
        &["過去"],
    ),
    rule("って", "う", WordType::INITIAL, WordType::GODAN, &["て形"]),
    rule("って", "つ", WordType::INITIAL, WordType::GODAN, &["て形"]),
    rule("って", "る", WordType::INITIAL, WordType::GODAN, &["て形"]),
    rule("いて", "く", WordType::INITIAL, WordType::GODAN, &["て形"]),
    rule("いで", "ぐ", WordType::INITIAL, WordType::GODAN, &["て形"]),
    rule("して", "す", WordType::INITIAL, WordType::GODAN, &["て形"]),
    rule("んで", "む", WordType::INITIAL, WordType::GODAN, &["て形"]),
    rule("んで", "ぶ", WordType::INITIAL, WordType::GODAN, &["て形"]),
    rule("んで", "ぬ", WordType::INITIAL, WordType::GODAN, &["て形"]),
    rule(
        "行って",
        "行く",
        WordType::INITIAL,
        WordType::GODAN,
        &["て形"],
    ),
    rule(
        "いって",
        "いく",
        WordType::INITIAL,
        WordType::GODAN,
        &["て形"],
    ),
    // Conditional godan forms.
    rule("えば", "う", WordType::INITIAL, WordType::GODAN, &["仮定"]),
    rule("けば", "く", WordType::INITIAL, WordType::GODAN, &["仮定"]),
    rule("げば", "ぐ", WordType::INITIAL, WordType::GODAN, &["仮定"]),
    rule("せば", "す", WordType::INITIAL, WordType::GODAN, &["仮定"]),
    rule("てば", "つ", WordType::INITIAL, WordType::GODAN, &["仮定"]),
    rule("ねば", "ぬ", WordType::INITIAL, WordType::GODAN, &["仮定"]),
    rule("べば", "ぶ", WordType::INITIAL, WordType::GODAN, &["仮定"]),
    rule("めば", "む", WordType::INITIAL, WordType::GODAN, &["仮定"]),
    rule("れば", "る", WordType::INITIAL, WordType::GODAN, &["仮定"]),
    // Godan imperatives. These are learner-facing command forms, not ichidan
    // stems; resolving them keeps titles like 走れ団子ちゃん on 走る.
    rule("え", "う", WordType::INITIAL, WordType::GODAN, &["命令"]),
    rule("け", "く", WordType::INITIAL, WordType::GODAN, &["命令"]),
    rule("げ", "ぐ", WordType::INITIAL, WordType::GODAN, &["命令"]),
    rule("せ", "す", WordType::INITIAL, WordType::GODAN, &["命令"]),
    rule("て", "つ", WordType::INITIAL, WordType::GODAN, &["命令"]),
    rule("ね", "ぬ", WordType::INITIAL, WordType::GODAN, &["命令"]),
    rule("べ", "ぶ", WordType::INITIAL, WordType::GODAN, &["命令"]),
    rule("め", "む", WordType::INITIAL, WordType::GODAN, &["命令"]),
    rule("れ", "る", WordType::INITIAL, WordType::GODAN, &["命令"]),
    // Potential/passive/causative.
    rule(
        "われる",
        "う",
        WordType::ICHIDAN,
        WordType::GODAN,
        &["可能", "受身"],
    ),
    rule(
        "かれる",
        "く",
        WordType::ICHIDAN,
        WordType::GODAN,
        &["可能", "受身"],
    ),
    rule(
        "がれる",
        "ぐ",
        WordType::ICHIDAN,
        WordType::GODAN,
        &["可能", "受身"],
    ),
    rule(
        "される",
        "す",
        WordType::ICHIDAN,
        WordType::GODAN,
        &["可能", "受身"],
    ),
    rule(
        "たれる",
        "つ",
        WordType::ICHIDAN,
        WordType::GODAN,
        &["可能", "受身"],
    ),
    rule(
        "なれる",
        "ぬ",
        WordType::ICHIDAN,
        WordType::GODAN,
        &["可能", "受身"],
    ),
    rule(
        "ばれる",
        "ぶ",
        WordType::ICHIDAN,
        WordType::GODAN,
        &["可能", "受身"],
    ),
    rule(
        "まれる",
        "む",
        WordType::ICHIDAN,
        WordType::GODAN,
        &["可能", "受身"],
    ),
    rule(
        "られる",
        "る",
        WordType::ICHIDAN,
        WordType::GODAN,
        &["可能", "受身"],
    ),
    rule(
        "わせる",
        "う",
        WordType::ICHIDAN,
        WordType::GODAN,
        &["使役"],
    ),
    rule(
        "かせる",
        "く",
        WordType::ICHIDAN,
        WordType::GODAN,
        &["使役"],
    ),
    rule(
        "がせる",
        "ぐ",
        WordType::ICHIDAN,
        WordType::GODAN,
        &["使役"],
    ),
    rule(
        "させる",
        "す",
        WordType::ICHIDAN,
        WordType::GODAN,
        &["使役"],
    ),
    rule(
        "たせる",
        "つ",
        WordType::ICHIDAN,
        WordType::GODAN,
        &["使役"],
    ),
    rule(
        "なせる",
        "ぬ",
        WordType::ICHIDAN,
        WordType::GODAN,
        &["使役"],
    ),
    rule(
        "ばせる",
        "ぶ",
        WordType::ICHIDAN,
        WordType::GODAN,
        &["使役"],
    ),
    rule(
        "ませる",
        "む",
        WordType::ICHIDAN,
        WordType::GODAN,
        &["使役"],
    ),
    rule(
        "らせる",
        "る",
        WordType::ICHIDAN,
        WordType::GODAN,
        &["使役"],
    ),
    // Godan potential contraction, e.g. 読める -> 読む.
    rule("える", "う", WordType::ICHIDAN, WordType::GODAN, &["可能"]),
    rule("ける", "く", WordType::ICHIDAN, WordType::GODAN, &["可能"]),
    rule("げる", "ぐ", WordType::ICHIDAN, WordType::GODAN, &["可能"]),
    rule("せる", "す", WordType::ICHIDAN, WordType::GODAN, &["可能"]),
    rule("てる", "つ", WordType::ICHIDAN, WordType::GODAN, &["可能"]),
    rule("ねる", "ぬ", WordType::ICHIDAN, WordType::GODAN, &["可能"]),
    rule("べる", "ぶ", WordType::ICHIDAN, WordType::GODAN, &["可能"]),
    rule("める", "む", WordType::ICHIDAN, WordType::GODAN, &["可能"]),
    rule("れる", "る", WordType::ICHIDAN, WordType::GODAN, &["可能"]),
];

const fn rule(
    from: &'static str,
    to: &'static str,
    from_type: WordType,
    to_type: WordType,
    reasons: &'static [&'static str],
) -> Rule {
    Rule {
        from,
        to,
        from_type,
        to_type,
        reasons,
    }
}

pub(super) fn deinflect(surface: &str) -> Vec<Candidate> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut queue = VecDeque::new();
    queue.push_back(Candidate {
        form: surface.to_string(),
        reasons: Vec::new(),
        word_type: WordType::INITIAL.union(WordType::INFLECTABLE),
    });
    seen.insert((
        surface.to_string(),
        WordType::INITIAL.union(WordType::INFLECTABLE),
    ));

    while let Some(candidate) = queue.pop_front() {
        if candidate.reasons.len() >= MAX_DEPTH {
            continue;
        }
        for rule in RULES.iter().chain(MASU_STEM_RULES).chain(GODAN_RULES) {
            if !candidate.word_type.intersects(rule.from_type) {
                continue;
            }
            let Some(stem) = candidate.form.strip_suffix(rule.from) else {
                continue;
            };
            let form = format!("{stem}{}", rule.to);
            if form == candidate.form || form.is_empty() {
                continue;
            }
            let mut reasons = rule
                .reasons
                .iter()
                .map(|reason| (*reason).to_string())
                .collect::<Vec<_>>();
            reasons.extend(candidate.reasons.iter().cloned());
            let next = Candidate {
                form,
                reasons,
                word_type: rule.to_type,
            };
            if !seen.insert((next.form.clone(), next.word_type)) {
                continue;
            }
            out.push(next.clone());
            if out.len() >= MAX_CANDIDATES {
                return out;
            }
            queue.push_back(next);
        }
    }

    out
}

pub(super) fn entry_matches_type(entry: &Entry, word_type: WordType) -> bool {
    entry.senses.iter().any(|sense| {
        sense
            .part_of_speech
            .iter()
            .any(|pos| pos_matches_word_type(pos, word_type))
    })
}

fn pos_matches_word_type(pos: &str, word_type: WordType) -> bool {
    (word_type.intersects(WordType::ICHIDAN) && pos.starts_with("v1"))
        || (word_type.intersects(WordType::GODAN)
            && (pos.starts_with("v5") || pos.starts_with("v4")))
        || (word_type.intersects(WordType::I_ADJ) && pos.starts_with("adj-i"))
        || (word_type.intersects(WordType::KURU) && pos == "vk")
        || (word_type.intersects(WordType::SURU) && (pos.starts_with("vs") || pos == "vk"))
        || (word_type.intersects(WordType::NOUN_VS) && pos.starts_with("vs"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn forms(surface: &str) -> Vec<String> {
        deinflect(surface)
            .into_iter()
            .map(|candidate| candidate.form)
            .collect()
    }

    #[test]
    fn deinflects_polite_and_past_forms() {
        let polite_forms = forms("飲みました");
        assert!(
            polite_forms.contains(&"飲む".to_string()),
            "{polite_forms:?}"
        );
        let negative_forms = forms("食べませんでした");
        assert!(
            negative_forms.contains(&"食べる".to_string()),
            "{negative_forms:?}"
        );
    }

    #[test]
    fn deinflects_recursive_auxiliary_chains() {
        let forms = forms("食べさせられたくなかった");
        assert!(forms.contains(&"食べる".to_string()), "{forms:?}");
    }

    #[test]
    fn deinflects_suru_stems() {
        let forms = forms("達し");
        assert!(forms.contains(&"達する".to_string()), "{forms:?}");
    }

    #[test]
    fn deinflects_contracted_game_dialogue_forms() {
        let continuous_forms = forms("走ってた");
        assert!(
            continuous_forms.contains(&"走る".to_string()),
            "{continuous_forms:?}"
        );
        let contracted_forms = forms("読んじゃった");
        assert!(
            contracted_forms.contains(&"読む".to_string()),
            "{contracted_forms:?}"
        );
    }

    #[test]
    fn deinflects_godan_imperatives() {
        let forms = forms("走れ");
        assert!(forms.contains(&"走る".to_string()), "{forms:?}");
    }

    #[test]
    fn deinflects_iku_onbin_forms() {
        let past_forms = forms("行った");
        assert!(past_forms.contains(&"行く".to_string()), "{past_forms:?}");
        let te_forms = forms("行って");
        assert!(te_forms.contains(&"行く".to_string()), "{te_forms:?}");
    }
}
