# Character Accuracy

Where mite's text accuracy comes from, where it currently stands, and why it
stops where it does. Latency and runtime mechanics live in
`docs/architecture.md` and `docs/performance.md`; eval metadata policy lives in
`docs/eval-metadata.md`. This document is about getting the *characters* right.

## Current state

Measured on the private eval corpus (414 labeled real 4K captures, scored
0.35 detection / 0.40 characters / 0.25 metadata):

```text
aggregate 96.18% | detection 96.08% | characters 98.73% | metadata 92.26%
(2026-06-09; up from 91.08% / 90.89% / 93.77% / 87.04% at the campaign start)
```

Character accuracy is the highest-leverage number of the three: a single wrong
character shifts every later token span in that line, so each character error
also costs roughly three tokens of metadata credit. Improving recognition pays
about 3x its face value; this is why most of the machinery below exists.

## What the pipeline does, from an accuracy standpoint

1. **Capture** delivers native-resolution frames; nothing downstream ever
   upscales lost pixels back.
2. **Detection** (PP-OCRv5 mobile, TensorRT FP16) runs at native 4K — small
   glyphs are not sacrificed to a downscale — and runs **twice**: the standard
   pass, plus a pass over a local-contrast luminance image (integral-image
   high-pass) that recovers faint text on translucent panels and grey-on-grey
   UI. Component boxes come from scanline connected components; candidates
   from both passes are deduped, plausibility-filtered, and capped at 256
   boxes with lowest-confidence-first truncation (never screen-position
   truncation: dense menus really do have 100+ real lines, and dropping the
   bottom of the screen was once the single largest accuracy loss).
3. **Recognition** crops each detected line from the *full-resolution* frame
   with 0.5x-line-height padding — the detector's probability map routinely
   clips a weak edge glyph (trailing 。/：, leading brackets), and the margin
   lets the recognizer read what the detector under-boxed without moving the
   scored box. Crops are batch-recognized at the model's 48 px input height
   and greedily CTC-decoded with per-glyph positions.
4. **Shape rescue** synthesizes ◇/◆ from empty-text near-square boxes via a
   geometric classifier (L1-ball mass, linear taper toward the tips,
   vertex/edge presence, crisp-edge sharpness). These bullets are real,
   labeled UI glyphs that the recognizer can never emit because they are not
   in the PP-OCR charset.
5. **Normalization** (`ort_engine/text.rs`, `text_corrections.rs`) repairs the
   recognizer's systematic confusions; see the next section.
6. **Filtering** (`ocr.rs`) drops what the detector should not have boxed:
   confidence floors, an ASCII-microtext rule, a short-ASCII confidence floor,
   a CJK microtext floor, id-like numeric noise, the TM logo mark. Merge rules
   rejoin lines the detector split at mid-line pauses (……, ——) and two-glyph
   wrapped tails (ム。 + continuation).
7. **Block analysis and the dictionary matrix** (`text_blocks.rs`,
   `dictionary/mod.rs`) sit downstream of character accuracy but are coupled
   to it: wrapped-line joining is gated on dictionary evidence, so a single
   misread character can prevent a join (基づ + く表示 fails if く is lost)
   and cascade into many token-level losses.
8. **Optional second opinion**: `models.fallback_recognizer_path` can load the
   PP-OCRv5 server recognizer (FP32) for lines the mobile model reads below
   0.75 confidence, accepting its read only above 0.92. Measured score-neutral
   on the eval corpus (see "Why we stop here"), shipped default-off.

## The accuracy levers, and why each exists

Roughly in pipeline order. Every lever was either measured to pay on the eval
corpus or inherited from earlier measured work; several near-misses that did
*not* pay are listed in the next section.

- **Native-4K dual-pass detection.** Recall on small and faint text. The
  low-contrast pass costs a second detector inference but its image build now
  overlaps the primary pass's GPU time.
- **Area-averaging downscale** for the optional low-resolution detector path:
  integrates all source detail, so thin strokes survive a shrink that bicubic
  would alias.
- **Score-ranked box cap.** A safety valve that can no longer silently delete
  panels.
- **0.5h recognition crop padding** with guards that keep pad-region junk from
  moving scored boxes or triggering the box-adjust heuristics.
- **Charset-gap shape rescue** for ◇/◆, gated hard enough (taper, vertices,
  edges, sharpness, same-row text) that blurred buttons, reticle icons, and
  oversized decorations are all rejected.
- **Contextual orthography rules**, each encoding a glyph confusion the
  recognizer makes systematically and a context where one reading is
  essentially impossible: kanji/katakana lookalikes between katakana
  (ハーモ二ー -> ハーモニー), katakana ヘ before hiragana is the particle へ,
  full-size ヨ/ユ after a palatalizable kana before more katakana is the small
  glyph (シヨック -> ショック), ASCII !?() become full-width in Japanese
  context, a lone digit after sentence-final 。 at line end is a neighboring
  list number, simplified-Chinese sibling glyphs map to the Japanese forms
  (齐 -> 斉, 测 -> 測, ...).
- **Word-level confusion-pair literals** (`text_corrections.rs`): corrections
  whose search string is a misread that cannot occur in correct text and whose
  replacement is an ordinary word or curated domain term (圧カ -> 圧力,
  マッブ -> マップ, 乗山 -> 乗霄山). This file predates the current pipeline
  and its standard is deliberate: a literal must encode a *glyph confusion in
  a word context*, never reconstruct the content of a specific screen.
- **Data-calibrated noise floors.** Legitimate short ASCII labels (HP, Lv.1,
  x5) recognize at 0.88+ while decal junk tops out at 0.86; no labeled
  Japanese line is under 15 px. The floors (0.87, 13.5 px) sit inside those
  margins. They are calibrated on this corpus and are the first constants to
  re-check if mite ever targets a second game.
- **Pinned TensorRT optimization profiles.** Rebuilding the detector engine
  with a wider dynamic-shape profile changes kernel tactic selection and
  measurably perturbs detection (~0.3 aggregate points) with no code change.
  Profiles are part of measured behavior; treat them as frozen and re-baseline
  if they must move.
- **The measurement loop itself.** Every accuracy-affecting change runs the
  eval subset (`eval-subset\`, seconds) and then the full corpus
  (`eval-corpus`, ~25 min), with per-detection A/B diffs between report
  directories. `scripts/analyze-eval-failures.ts` and
  `scripts/attribute-eval-loss.ts` attribute the loss in aggregate points by
  cause; subset gains have flipped sign at corpus scale more than once, so a
  subset result alone is never trusted.

## Why we stop here (the overfitting boundary)

The remaining ~3.8 points decompose into three pots, and the legitimate
tooling for each is exhausted:

1. **Recognizer capability on hard pixels** (~1.5-2 points including the
   metadata cascade): faint translucent-panel garbles, sub-20 px glyph soup.
   This is the mobile model's ceiling, not a post-processing problem.
2. **Box-geometry noise inside the eval's own tolerance design** (~0.9):
   detector boxes and human-drawn label bounds disagree by fractions of a
   glyph in ways both defensible. Pixel-band auditing already corrected every
   label that was *provably* wrong (41 of them; see `eval/LABEL-CHANGES.md`).
3. **A diffuse one-off misread tail** across 360+ captures.

Seventeen measured experiments say the remaining levers do not pay, and the
reason is structural: **the eval labels co-evolved with this reader.**
Annotators accepted the pipeline's output wherever it matched the pixels, so
on genuinely ambiguous glyphs the labels encode what *this* pipeline reads.
Any change to how ambiguous pixels are read — server detection or recognition
(FP16 and FP32), per-line server fallback, contrast-stretch retries, bicubic
crop upscaling, 1.5x super-native detection, six crop/box geometry variants,
and a lexicon-adjudicated CTC runner-up swap (built, unit-tested, measured,
reverted) — shifts those reads in both directions and nets approximately zero
against these labels.

What *would* move the number is exactly what we refuse to do:

- **Capture-specific correction literals.** A few hundred entries
  reconstructing individual screens' text would push the eval score up
  without making mite read anything better. The correction file's standard
  (glyph confusions in word contexts only) is the line; entries that crossed
  it were removed even at a small measured cost.
- **Regenerating labels from runtime output.** This maximizes the score by
  construction and destroys the eval's value as ground truth.

In short: past this point, raising the eval number and raising real accuracy
stop being the same project.

## Next steps if 99% becomes worth it

The honest path runs through the model, not more post-processing:

1. **Fine-tune the PP-OCRv5 mobile recognizer on synthetic Japanese game
   text.** Render JMdict vocabulary and game-style strings in game-adjacent
   fonts over captured-background crops, with the degradations that actually
   hurt: translucency compositing, low contrast, sub-20 px sizes, bloom and
   depth-of-field blur. Fine-tune in PaddleOCR (the rec training recipe is
   maintained), export through paddle2onnx, and rebuild the TensorRT engine.
   This attacks pot 1 directly — the only pot big enough to reach 99%.
   Validation must be the full-corpus A/B with per-detection diffs, expecting
   some label churn where labels encode the old reader's choices on ambiguous
   pixels (re-audit those through `examples/relabel_eval_tokens.rs` rather
   than hand-editing).
2. **A detector fine-tune or successor** for isolated single glyphs (ン, く as
   a wrapped line by itself) if pot 2's misses matter after step 1.
3. **Tolerance-model re-annotation** only if box-geometry noise must fall:
   re-derive per-label `bounds_tolerance` from measured detector jitter
   instead of hand-drawn margins. This changes the eval contract, so it is a
   deliberate, documented decision — not a tuning knob.

A useful budget estimate: the recognizer fine-tune is GPU-days of training
plus a synthetic-data pipeline, against an expected 1.5-2 point gain. Nothing
smaller closes the gap; everything smaller has been measured.
