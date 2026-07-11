# Eval Metadata Policy

Japanese often allows more than one valid way to split and define the same
sentence - is である one word or two? Is して a verb or a connector here?
When several analyses are defensible, mite always teaches the same one: the
stable, commonly taught interpretation a learner can act on, with nuance left
to notes. This page is the rulebook for those choices. If you have ever
wondered why mite's popup picked a particular dictionary entry, the answer is
here or in the in-code decision matrix this page governs.

Mite's eval labels are not only OCR correctness fixtures. They also encode the
lookup behavior the overlay should present to a Japanese learner. That makes
metadata labels part of the product contract: a passing eval should mean the
user sees a stable, useful primary explanation, not merely that the runtime found
some defensible dictionary entry.

## Priority

The primary label should be beginner-safe and consistent. Japanese often permits
several valid analyses for the same surface form, especially around auxiliaries,
light verbs, nominalizers, kana spellings, and wrapped UI text. The eval should
choose one primary interpretation by rule and leave nuance for notes or future
alternate-analysis UI.

Use this order when adding or changing metadata rules:

1. Preserve the visible text exactly. OCR surfaces, punctuation, glyph variants,
   and line breaks are scored strictly.
2. Choose the primary lookup a beginner should act on. Prefer a stable,
   commonly taught interpretation over a rarer technically valid one.
3. Follow open dictionary and morphology data when it supports the learner-safe
   interpretation. JMdict senses, JMdict `misc` tags such as `uk`, and
   Lindera/IPADIC morphology are evidence, not automatic final answers.
4. Keep scoring deterministic. Similar contexts should receive the same
   `dictionary_form`, `category`, POS tags, furigana, note, and gloss shape.
5. Put complexity in notes or alternate analyses. Do not destabilize the primary
   label every time a form has a second plausible reading.

## Matrix In Code

The concrete decision matrix belongs in the dictionary/eval code, not in this
document. The matrix is executable product policy: it should be covered by unit
tests and used by eval-label drafting so new annotations inherit the same
choices as the overlay.

When adding a matrix entry, write the code comment near the rule and link it
back to this document. The comment should explain which learner-facing ambiguity
the rule resolves and what external data or corpus pattern motivated it. Avoid
copying a long table into docs; the table will drift from the implementation.

## Current Decisions

The corpus historically labeled several constructions two different ways in
different annotation passes. The matrix in code now pins one convention per
construction (each with a unit test in `src/dictionary/mod.rs`), and the
nonconforming labels were regenerated; `eval/LABEL-CHANGES.md` records every
change. The pinned conventions:

- である/であり split into copula で (だ · 連用形) plus ある/あり; でも before
  an ある-family token splits into copula で plus focus particle も.
- て + いる stays split, with いる as the progressive auxiliary (補助動詞);
  ていた remains one merged past-progressive token.
- Truncated list rows ending in the ASCII "..." marker fuse the clipped
  fragment with the marker into one unknown token (メモワー..., 叫...), except
  a single clipped katakana character, which stays its own fragment. The …
  leader is prose trail-off and never fuses.
- Date values such as 2月2日 are one unscored value token.
- Bracket and punctuation tokens are always unknown layout tokens, never
  dictionary words.
- な after a na-adjective stem (or starting a wrapped line) is the attributive
  copula だ (体言接続).

### Canonical Spellings

`dictionary_form` should be the form a learner should look up and learn as the
primary spelling. JMdict headword order is useful, but entries marked
usually-kana (`uk`) should prefer the kana form as the primary label. Kanji
spellings can be surfaced as alternate forms or notes.

This applies to high-frequency forms such as `できる`. The eval should not teach
that a usually-kana word must be written in kanji just because a morphological
analyzer or dictionary headword selected that spelling.

### Function Words And Auxiliaries

Function words should be labeled by their role in context. A token such as
`して`, `いる`, `ない`, or `の` can be a content word, auxiliary, particle, or
nominalizer depending on nearby tokens. The matrix should make that contextual
choice explicit.

For the current token model, a visible `して` after a suru-capable nominal is
scored as the te-form of `する`, with a learner note that exposes the connective
role. A future richer UI may render a decomposition such as `し + て`, but the
primary scored label remains one stable token-level interpretation.

High-frequency kana homographs should prefer the interpretation a learner is
most likely to need in game text. Examples include `もの` as the thing/object
sense rather than the person sense, `いい` as the ordinary "good/OK" adjective,
`ください` as a request auxiliary, and kana past forms such as `した`/`いた`
over incidental noun homographs unless nearby context clearly selects the noun.

### Wrapped Fragments

Line-wrapped UI text should be analyzed by stitchability. If adjacent visible
chunks reconstruct a real dictionary word, the fragment should carry the full
word's metadata so hovering the continuation still teaches the word. If no
neighbor completes the word, the fragment remains an unknown surface token.

This avoids both false unknowns for ordinary wrapped words and false dictionary
claims for isolated OCR fragments or clipped UI remnants.

### Domain Terms

Game-specific terms can override generic dictionary behavior when JMdict would
produce a misleading popup. Domain terms should stay narrow, exact, and
documented in code. The eval should reward domain popups only when they help the
learner understand the visible game text better than a generic dictionary split.

Suppressing a generic dictionary lookup is safest when the visible text contains
a curated lexical gate. For example, a clipped title continuation after a known
domain event name can remain unknown because the in-line event name explains why
the ordinary dictionary sense would mislead. Do not suppress a common word only
because the eval region or UI slot implies a proper name; that signal is not
available to the runtime lookup. Bare one-character names such as `角` should
therefore stay as known generic words unless a visible domain antecedent or a
future game dictionary provides a safer rule.

## Eval Authoring

Use the eval UI or `draft_expected_detection` path to rebuild metadata whenever
possible. Manual metadata edits are allowed, but they should follow the same
matrix as the runtime. If a current runtime result violates this policy, fix the
runtime or note the mismatch instead of copying it into labels as precedent.
Character and line placement follows [eval geometry](eval-geometry.md); metadata
drafting does not manufacture character positions.

Residual disagreement should be explicit:

- If the label is correct and runtime is wrong, keep the label and let eval fail.
- If the matrix has no rule for the case, add or clarify a rule before mass
  relabeling.
- If the form is genuinely ambiguous, keep the stable primary interpretation and
  record the alternate analysis in note-oriented UI or future non-scored fields.
