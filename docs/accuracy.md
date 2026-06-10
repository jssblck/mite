# Character Accuracy

How accurately mite reads Japanese text off the screen, why the pipeline is
built the way it is, and where the limits are. The first section is for
users; the rest is for people working on the code. Latency and runtime
mechanics live in `docs/architecture.md` and `docs/performance.md`; the
dictionary-label policy lives in `docs/eval-metadata.md`.

## For users: how much should you trust what mite shows you?

Mite reads the text in your game window with on-device OCR (optical
character recognition), then looks the words up in a dictionary. OCR is very
good but not perfect, so it helps to know what its mistakes look like.

Measured against several hundred hand-checked real 4K game screenshots:

- About **94 of every 100 text lines are read perfectly**, character for
  character.
- Counting individual characters, roughly **1 in 80 is wrong or missing**.
- Mistakes are not evenly spread. They concentrate in **small text** (tiny
  stat numbers, fine print), **faint text** (grey-on-grey, text on
  see-through panels), and **decorated text** (stylized titles, glowing or
  blurred captions). Ordinary dialogue and menu text is read very reliably.

When a misread happens, it usually shows up in one of two ways:

- **The word shows as unknown** (no dictionary entry). A wrong character
  usually breaks the word, and mite prefers showing "unknown" over guessing.
  This failure is visible, which is the safer kind.
- **The popup shows a real word that does not fit the context.** This is the
  misleading kind, and it is rarer: it needs the misread to accidentally
  spell a different real word. Glyph pairs that look nearly identical
  (katakana カ and the kanji 力, katakana ニ and the kanji 二) cause most of
  these, and the pipeline already corrects the common cases by context.

A practical rule of thumb: **if a definition seems unrelated to what is
happening on screen, suspect the reading before you suspect your Japanese.**
Hover the word again after the text re-renders, or check a neighboring line;
transient misreads usually do not repeat. Be most skeptical of tiny or
decorated text, and most trusting of plain dialogue.

One more honesty note: even when every character is read correctly, choosing
*which* dictionary entry to show involves judgment calls (Japanese allows
several valid analyses of the same text). Mite always picks one stable,
commonly taught interpretation rather than the most exotic one; that policy
is documented in `docs/eval-metadata.md`.

## What the pipeline does

Each stage exists to protect a specific kind of accuracy:

1. **Capture** delivers native-resolution frames. Nothing downstream can
   recover pixels lost here, so nothing is scaled before OCR.
2. **Detection** (PP-OCRv5 mobile) finds text-line boxes at native 4K and
   runs twice per frame: a standard pass, and a pass over a local-contrast
   transform of the frame that makes faint glyphs on translucent panels
   visible to the detector. Candidates from both passes are deduplicated and
   filtered for plausibility. Dense menu screens contain more real text
   lines than any fixed cap, so when the box cap binds it drops the
   lowest-confidence boxes — a full panel of real text can never silently
   vanish because of where it sits on screen.
3. **Recognition** crops each detected line from the full-resolution frame
   with half-a-line-height of margin. The detector's probability map tends
   to under-box weak edge glyphs (a trailing 。, an opening bracket); the
   margin lets the recognizer read them anyway, while the reported box stays
   the detector's so geometry remains stable.
4. **Shape rescue** handles the diamond bullets ◇ and ◆, which the games use
   as real text but which are absent from the recognizer's character set. A
   geometric classifier (diamond mass profile, linear taper toward the tips,
   all four vertices present, crisp edges, text on the same row) synthesizes
   them from boxes the recognizer returned empty. The gates exist to reject
   look-alikes: blurred buttons, reticle icons, oversized map decorations.
5. **Normalization** repairs the recognizer's systematic confusions; the
   standing rules are described below.
6. **Filtering** removes what should never have been boxed: background
   decals, microscopic pseudo-text, logo marks. Merge rules rejoin lines the
   detector splits at mid-line pauses (……, ——) and at short wrapped tails.
7. **Word analysis** (line grouping, dictionary segmentation) sits
   downstream but is coupled to character accuracy: wrapped-line joining
   requires dictionary evidence that a word crosses the line break, so one
   wrong character can prevent a join and cost several word lookups. This
   coupling is why character accuracy is the highest-leverage number in the
   whole system: a character error costs roughly three times its face value
   once word-level effects are counted.
8. **Optional second opinion**: `models.fallback_recognizer_path` can load
   the heavier PP-OCRv5 server recognizer for lines the primary reads with
   low confidence. It is off by default; see "Approaches tried and
   rejected" for why it does not earn its cost on the eval corpus.

## Standing design rules

These are the rules the pipeline follows and the reasoning behind them.
They hold regardless of who is editing the code; change them only with
fresh corpus measurements (see "How accuracy changes are validated").

- **Detect at native resolution; never trade pixels for speed on the
  detection path.** Recall on small glyphs is bought with optimization
  elsewhere (see `docs/performance.md`), not by shrinking the input. When a
  downscaled detector path is explicitly configured, it uses area-averaging
  resampling, which preserves thin strokes that bicubic filtering aliases
  away.
- **Recognition crops always come from the full-resolution frame**, whatever
  the detector saw.
- **Contextual orthography rules fix only impossible readings.** Each rule
  encodes a glyph confusion the recognizer makes systematically, applied
  only in contexts where one reading essentially cannot occur in Japanese:
  a kanji look-alike between two katakana is the katakana (ハーモ二ー →
  ハーモニー); katakana ヘ before hiragana is the particle へ; full-size
  ヨ/ユ between a palatalizable kana and more katakana is the small glyph
  (シヨック → ショック); ASCII !?() become full-width inside Japanese text;
  simplified-Chinese sibling glyphs map to their Japanese forms. Rules that
  would need to guess between two plausible readings do not belong here.
- **Correction literals encode glyph confusions in word contexts, never
  screen content.** An entry's search string must be a misread that cannot
  occur in correct text, and its replacement an ordinary word or curated
  domain term (圧カ → 圧力, マッブ → マップ, 乗山 → 乗霄山). Entries that
  reconstruct what a particular screen says are forbidden; see "The
  overfitting boundary".
- **Noise floors are calibrated with margins, and are corpus-derived.**
  Legitimate short ASCII labels (HP, Lv.1, x5) recognize well above the
  floor that removes decal junk, and no real Japanese line in the corpus is
  near the microtext height floor. These constants encode facts about one
  game's UI; they are the first thing to re-verify if mite targets another
  game.
- **TensorRT optimization profiles are part of measured behavior.**
  Rebuilding an engine with a different dynamic-shape profile changes
  kernel selection and shifts detection output slightly but measurably,
  with no code change. Treat profile constants as frozen; re-baseline the
  corpus if they must move.
- **Domain terms are curated, narrow, and exact** (`dictionary/mod.rs`).
  They exist so game-specific names neither fragment into misleading
  dictionary pieces nor get invented where the evidence is thin.

## How accuracy changes are validated

The private `eval\` submodule holds the labeled corpus; aggregate scoring
weighs detection 0.35, characters 0.40, word metadata 0.25. Any
accuracy-affecting change runs:

1. the iteration subset (`eval-subset\`, copied captures, fast), then
2. the full corpus (`cargo run -- eval-corpus`, ~25 minutes), then
3. a per-detection diff between the before/after report directories.

`scripts/analyze-eval-failures.ts` and `scripts/attribute-eval-loss.ts`
aggregate failures and attribute the loss in aggregate points by cause.
Subset results alone are never trusted — subset gains have inverted at
corpus scale. Label corrections go exclusively through the evidence-gated
tools (`examples/audit_label_bounds.rs`, `examples/relabel_eval_tokens.rs`)
and every change is recorded with its evidence in `eval/LABEL-CHANGES.md`.

## Approaches tried and rejected

These were each implemented (or configured), measured on the corpus, and
rejected. They are recorded so future work does not re-try them blindly.
The per-run reports live under `target\eval\` during a campaign and the
implementations are recoverable from git history.

| Approach | Why it seems attractive | Why it fails here |
|---|---|---|
| Server detection / recognition models (FP16 and FP32), whole-corpus | Bigger models read hard text better in general | Scores at or below mobile on this corpus; FP16 additionally overflows. See the co-evolution note below |
| Per-line server-recognizer fallback on low-confidence lines | Pay the big model only where the small one struggles | Score-neutral; kept as a default-off option for use outside the corpus |
| Contrast-stretch retry of low-confidence lines | Faint panels compress the luminance range the model trained on | Model confidence is not a reliable quality signal across preprocessing changes; ungated it also revives junk past the noise filters |
| Lexicon-guided CTC decoding (runner-up glyph swaps adjudicated by the dictionary) | Recovers near-tie misreads the greedy decoder discards | Net zero to slightly negative; the labels already encode this reader's choices on ambiguous glyphs |
| Detector box growth / crop-extension geometry (several variants) | Boxes measurably under-cover edge glyphs | Label bounds were drawn around this detector's behavior; every geometric change loses more bounds credit than it gains in text |
| Super-native (1.5x) detection input | Small isolated glyphs get more pixels | Box geometry shifts against labels; the widened TensorRT profile alone perturbs scores |
| Detector threshold / morphology-radius tuning | Cheap knobs | Within noise at best; closing radii merge neighboring UI elements |
| Bicubic upscaling of small recognition crops | Sharper input for tiny text | The model expects its training-time resampling; sharper input reads slightly worse |

The unifying finding: **the eval labels co-evolved with this reader.**
Annotators accepted the pipeline's output wherever it matched the pixels,
so on genuinely ambiguous glyphs the labels record what this pipeline
reads. Changing how ambiguous pixels are read — a different model, decoder,
or preprocessing — moves those reads in both directions and nets roughly
zero against these labels, even when the change is a genuine improvement in
the abstract. Real gains against this corpus come from the other side:
recovering text the pipeline missed entirely, and fixing labels that are
provably wrong.

## The overfitting boundary

The eval score could be pushed higher in ways that would make the tool no
better — or worse — for users. Two are explicitly off-limits:

- **Capture-specific correction literals.** Hundreds of entries
  reconstructing individual screens' text would raise the eval score while
  teaching the pipeline nothing general. The correction file's standard
  (glyph confusions in word contexts only) is the line.
- **Regenerating labels from pipeline output.** This maximizes the score by
  construction and destroys the corpus's value as ground truth. Labels
  change only with proof: pixel measurements, sibling-capture evidence, or
  documented convention alignment, all logged in `eval/LABEL-CHANGES.md`.

Past this boundary, raising the eval number and raising real accuracy stop
being the same project. The residual gap (see the snapshot below)
decomposes into recognizer capability on faint and tiny glyphs, small
box-geometry disagreements inside the eval's own tolerance design, and a
thin tail of one-off misreads — none of which post-processing can fix
honestly.

## The path to higher accuracy

The honest route runs through the model, not more post-processing:

1. **Fine-tune the recognizer on synthetic Japanese game text.** Render
   dictionary vocabulary and game-style strings in game-adjacent fonts over
   captured-background crops, with the degradations that actually hurt:
   translucency compositing, low contrast, sub-20 px glyph sizes, bloom and
   depth-of-field blur. Train with PaddleOCR's maintained recognizer
   recipe, export via paddle2onnx, rebuild the TensorRT engine. This is the
   only lever aimed at the largest residual pot. Expect some label churn
   where labels encode the old reader's choices; re-audit those through
   `examples/relabel_eval_tokens.rs` rather than hand-editing.
2. **A detector fine-tune** for isolated single glyphs (a lone ン or く on
   its own wrapped line), if those misses still matter afterward.
3. **Tolerance-model re-annotation** — deriving per-label bounds tolerance
   from measured detector jitter instead of hand-drawn margins — only as a
   deliberate, documented change to the eval contract.

Budget honestly: the recognizer fine-tune is a synthetic-data pipeline plus
GPU-days of training for an expected 1.5–2 aggregate points. Smaller
efforts have all been measured and do not close the gap.

## Measurement snapshot

2026-06-09, 414-capture corpus, 8,282 labeled lines:

```text
aggregate 96.18% | detection 96.08% | characters 98.73% | metadata 92.26%
lines read perfectly: 94.0%
line error rate, small text (<22 px): 8.7% | normal text: 5.8%
```
