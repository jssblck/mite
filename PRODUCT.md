# Product

## Register

product

## Surface

Mite has no web frontend. Its interface is a native Windows layered,
click-through Win32 overlay drawn with GDI, plus hover definition popups and an
optional latency HUD. "Design" here means the overlay's visual and interaction
craft -- popup legibility, the word-category color system, furigana and gloss
layout, contrast over arbitrary game backgrounds, and how little the overlay
intrudes on the scene underneath. Web-specific impeccable steps (DESIGN.md scan,
`live` browser mode, CSP) do not apply; the design tokens live in
`src/win32_overlay/style.rs` and the popup-content logic in `src/hover/`.

## Users

Japanese learners and players reading Japanese text in games and visual novels
on Windows (optimized for NVIDIA / TensorRT). Their context is active: they are
in a game, mid-scene, trying to read or comprehend a line without breaking
immersion or alt-tabbing to a separate dictionary. The job to be done is "tell
me what this word means, right here, right now, without making me lose the
moment." Latency and lookup accuracy are felt directly -- a slow or wrong popup
is worse than none.

## Product Purpose

A local, low-latency OCR overlay that turns on-screen Japanese into hoverable
dictionary lookups. It captures the target window, runs PP-OCRv5 detection and
recognition through ONNX Runtime, segments recognized text into dictionary words
with Lindera, looks them up in JMdict with JPDB frequency ranking, and paints a
transparent click-through overlay with per-word highlights and hover
definitions. Everything runs on the local machine -- no network round-trip,
no cloud OCR. Success is a full 4K pass staying near ~200 ms p95 while the
overlay sits unnoticed until the user wants a reading, and the definition that
appears is the right one.

## Brand Personality

An invisible precision instrument. The overlay's voice is quiet, exact, and
deferential to the game underneath. It does not announce itself, decorate
itself, or compete for attention; it earns trust by being fast, correct, and
gone the instant it is not needed. Three words: precise, unobtrusive, instant.
The emotional goal is calm confidence -- the user trusts that when they look, the
answer will already be there and right.

## Anti-references

- **Heavy chrome that hides the game.** No large opaque panels, thick decorative
  borders, or persistent furniture that obscures the scene being read. The
  player's text and art come first; the overlay is a thin layer over it, not a
  window in front of it.
- **Childish or gamified styling.** No badges, mascots, achievement-toast
  flourishes, streak counters, or cartoon embellishment. This is a reading tool
  for adults studying or playing, not a game layer of its own.
- **A dense data dashboard.** The HUD and metrics exist for diagnosis, not for
  daily presence. The default reading experience is one word and its definition,
  not rows of stats and controls competing with the text.
- (Implied) A generic translucent-glass desktop widget. Contrast and legibility
  win over decorative blur; the popup is a solid, readable panel, not a frosted
  floating card.

## Design Principles

1. **Deference to the scene.** The overlay's first duty is to not get in the
   way. Footprint stays minimal; presence is earned per-hover, not assumed. When
   in doubt, show less.
2. **Right-and-instant beats rich.** A correct definition that appears with no
   perceived delay is the whole product. Never trade tail latency or lookup
   accuracy for visual richness. Tail latency (p95/p99) is the felt metric, not
   averages.
3. **Legible over any background.** The popup must read over bright, dark, busy,
   or plain game frames alike. Contrast and a solid panel are non-negotiable;
   translucency and decoration are not.
4. **The color system carries meaning, not decoration.** Word-category colors
   (particle, noun, verb, adjective, ...) are an information channel. Keep them
   consistent, distinguishable, and tied to grammar -- never recolored for
   aesthetics alone.
5. **Diagnostics stay opt-in.** The HUD, timing graphs, and metrics are tools for
   making the overlay good, surfaced on request (`--hud`, `--metrics-interval-secs`),
   never part of the resting reading surface.

## Accessibility & Inclusion

- **High-contrast popups anywhere (primary).** Popup text, furigana, and gloss
  must stay readable over arbitrary game backgrounds. The panel is solid and
  dark by default (`COLOR_PANEL_BG`), with body text near the ink end of the
  ramp; do not lighten text toward elegance at the cost of contrast. Aim for
  WCAG AA-equivalent legibility (>= 4.5:1 body, >= 3:1 large text) against the
  panel.
- **Reading-support legibility (primary).** Furigana and gloss text remain crisp
  and large enough to read at a glance mid-scene. Reading support is the point of
  the product, not a secondary affordance; never shrink it to fit more in.
- **Secondary, worth keeping in mind.** Word-category colors should stay
  distinguishable for common color-vision deficiencies (the
  amber/blue/green/orange/purple set in `category_rgb`), and motion should stay
  calm and non-distracting so it never pulls the eye off the game.
