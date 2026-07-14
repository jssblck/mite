# Eval Geometry

Eval geometry is the target for OCR placement, hover hit testing, token
underlines, and furigana anchors. Labels describe where text is rendered in the
original frame. Detector output is diagnostic evidence, not ground truth.

The measurement lives in `src/text_geometry.rs` and is shared by both sides of
the eval: label tooling measures character cells inside annotator-drawn
regions, and the live pipeline snaps each recognized line's box and character
centres to the same measurement after recognition (guarded, so unreliable
measurements leave the detector box untouched). Runtime output and labels
therefore agree by construction wherever the pixels are unambiguous.

## Character Cells

Each character rectangle follows its rendered typographic cell. Its horizontal
edges sit in the visual gaps between adjacent glyphs. Its vertical edges follow
that character's visible extent within the line, including the lower placement
of punctuation. The detection rectangle is the union of its character cells.

Equal-width subdivision of a line rectangle is invalid. It loses proportional
Latin widths, punctuation placement, side bearings, and trailing space. Those
errors accumulate across a line and move token underlines and furigana away from
the text they describe.

Schema 2 records the source in `character_geometry`:

- `pixel_gradient_v1`: measured from the original image by the eval UI or
  `reannotate_character_bounds`.
- `manual`: placed by an annotator against the original pixels when automatic
  measurement could not produce a reliable partition.

## Re-annotation

Audit without changing labels:

```powershell
cargo run --release --example reannotate_character_bounds -- `
  --root eval `
  --out target\eval\character-geometry-audit.json `
  --force
```

The tool measures luminance-gradient rows and columns inside each human-drawn
text region. It finds the dense line band, locates inter-glyph valleys near the
expected typographic advance, and measures vertical extent per character. A
minimum line-height constraint prevents a faint antialiased edge from
collapsing the box. Measurements below the confidence floor are left unchanged
for manual review.

Geometry provenance makes the operation idempotent. Existing measured or
manual detections are skipped unless `--force` is supplied.

## Review

Preview overlays use red for the previous geometry and green for the proposal:

```powershell
cargo run --release --example reannotate_character_bounds -- `
  --root eval\collection\capture-123 `
  --out target\eval\capture-123-geometry.json `
  --preview-dir target\eval\capture-123-preview
```

Review punctuation, short fragments, faint text, mixed ASCII and Japanese, and
text over strong UI borders. Structural audits then confirm that every
character remains inside its detection and that token spans still cover the
same text.
