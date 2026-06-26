# AGENTS.md

Guidance for coding agents working in this repository.

## Project Overview

Mite is a Windows-first, local, low-latency OCR overlay for Japanese text in games
and visual novels. The main product command is `watch`: it captures a target
window, runs PP-OCRv5 detector and recognizer ONNX models through ONNX Runtime,
segments recognized Japanese text into dictionary words, looks those words up in
JMdict, and draws a transparent click-through Win32 overlay with hover
definitions.

The project is Rust 2024 and uses `cargo`. The live path is optimized for
Windows/NVIDIA systems with a TensorRT -> CUDA -> CPU fallback chain. Keep
latency and lookup accuracy visible whenever you touch capture, OCR, smoothing,
dictionary segmentation, or overlay presentation.

## Source Of Truth

- `README.md` is the user-facing overview and quick start.
- `docs/architecture.md` documents the current runtime boundaries, latency
  target, TensorRT/CUDA fallback chain, and smoothing policy.
- `docs/local-windows.md` documents local setup, model/runtime files, watch
  usage, and troubleshooting.
- `docs/models.md` documents model provenance and the mobile-vs-server tradeoff.
- `docs/accuracy.md` documents where character accuracy comes from: the
  pipeline's accuracy levers, the measured overfitting boundary, and the
  fine-tuning path beyond it.
- `docs/performance.md` documents the before/after evidence expected for latency
  or throughput changes.
- `docs/eval-metadata.md` documents the learner-first metadata policy for eval
  labels, including why the concrete interpretation matrix lives in code and how
  to handle ambiguous Japanese lookup cases consistently.
- `docs/future/pure-gpu.md` is exploratory design context, not committed roadmap.
- `PRODUCT.md` is the strategic design context for the overlay surface: register
  (product), users, purpose, the "invisible precision instrument" personality,
  anti-references, design principles, and accessibility anchors (high-contrast
  popups, reading-support legibility). Consult it before changing overlay
  presentation, the popup/HUD palette in `src/win32_overlay/style.rs`, or the
  hover popup content in `src/hover/`. It is a native Win32 surface; there is no
  web frontend, so web-oriented design tooling does not apply.
- `THIRD_PARTY_NOTICES.md` documents third-party software, models, dictionaries,
  and data that need explicit attribution or license awareness.
- `eval\AGENTS.md` documents the private eval data submodule and how to manually
  load the data-specific annotation skill when that submodule is initialized.
- `app\README.md` documents the desktop app: the "mite home" layout, the launch
  contract, the shared design tokens, and the build/dev commands.
- `docs\releases.md` documents tag-based versioning and the release pipeline that
  the desktop app installs and updates from.
- `LICENSE` is the repository license. `CLAUDE.md` files are intentionally bare
  `@AGENTS.md` imports of their sibling so agent guidance does not drift
  between agent surfaces; keep all real guidance in the `AGENTS.md` files.

## Build, Test, And Run

Core checks:

```powershell
cargo fmt --check
cargo test
cargo clippy --all-targets -- -D warnings
```

Local precommit:

```powershell
.\scripts\bootstrap-dev.ps1 -HooksOnly
.\scripts\precommit.ps1
.\scripts\precommit.ps1 -IncludeEval
```

`precommit.ps1` scopes itself to the staged changes: the Rust suite (fmt,
nudge, test, clippy) is skipped when a commit touches only `site\**`, and the
site checks (`astro check`, `vitest run`) run only when `site\**` changed and
its `node_modules` is present. Run by hand with nothing staged, it runs both.

First-time Windows setup:

```powershell
.\scripts\bootstrap-dev.ps1
.\scripts\bootstrap-dev.ps1 -IncludeEvalData
```

`bootstrap-dev.ps1` does not install the NVIDIA GPU runtime: Mite never
downloads, hosts, bundles, or installs NVIDIA binaries, and that applies to the
developer tooling too. Install the runtime yourself from NVIDIA (the CUDA
Toolkit, cuDNN, and TensorRT 10.x) or from the pinned pip wheels, make it
discoverable on `PATH`, and verify with `cargo run -- doctor`. See
`docs\local-windows.md`.

Product commands:

```powershell
cargo run -- list-windows
cargo run -- list-windows --json --thumbnails   # what the desktop picker calls
cargo run -- watch
cargo run -- watch --title "Window title" --auto
cargo run -- watch --hud
cargo run -- watch --metrics-interval-secs 5
cargo run -- watch --auto-eval-capture   # auto-save a raw frame on each new scene (eval fixtures)
cargo run -- eval --image eval\collection-name\capture-<ts>\underlying.png --labels eval\collection-name\capture-<ts>\eval.json --out target\eval\capture-<ts>.json
cargo run -- eval-corpus --root eval --out target\eval\corpus-summary.json --out-dir target\eval\corpus --allow-failures
```

Use `eval` against manually labeled real captures as the OCR plus dictionary
regression guard. Use `watch --metrics-interval-secs` or `--hud` as the latency
acceptance loop. On the default native-4K TensorRT path a typical full 4K pass
should stay near ~200 ms p95 (~270 ms p99 on the densest menu frames); the
optional low-res detector path runs ~100 ms p95. Current reference numbers live
in `docs/performance.md`. Watch p95/p99, not just averages.

## Local Artifacts

The following are local/runtime artifacts and should not be committed:

- `models\`
- `cache\`
- `target\`
- `mite.toml`
- `.gpu-runtime\`
- `.venv-models\`
- `.env`

Model files and dictionaries are acquired with
`scripts\bootstrap-dev.ps1 -ModelsOnly`.
The TensorRT/CUDA/cuDNN runtime is installed by the developer from NVIDIA (or the
pinned pip wheels) and made discoverable on `PATH`; the repository never fetches
or stages it. `.gpu-runtime\` is only an optional local drop-in folder `doctor`
will search. See `docs\local-windows.md`.
Real-image eval captures live under the private `eval\` submodule, usually as
`eval\collection-name\capture-<ts>\` bundles.
Each scored capture should keep the original full-frame `underlying.png`, the
raw `capture.json` context, and a sibling human-authored `eval.json`. Mite does
not generate labels; the eval file is the source of truth for expected detection
bounds, exact per-character labels, and lookup/popup metadata. Detection bounds
are scored with jitter tolerance and may use per-label `bounds_tolerance`; text
and metadata remain strict.

Corpus-specific annotation tooling is stored in the private data submodule, not
in this source repository. When that work is needed, manually load
`eval\.agents\skills\build-mite-eval-from-image\SKILL.md` before labeling or
auditing captures.

## Private Eval Disclosure

Private eval data may contain third-party IP content. Do not mention private
corpus names, game/product/source names, collection directory names, capture
contents, screenshots, transcribed visible text, character or place names, or
other IP-bearing details in public-facing artifacts such as PR titles, PR
descriptions, issues, commit messages, release notes, docs, or chat summaries.
Use neutral terms such as "private eval corpus" or "private eval data" and keep
validation evidence to non-sensitive command outcomes, aggregate counts, and
pass/fail status. Local file paths and commands that include private corpus
directory names may be used in private terminal workflows, but redact or
generalize them before publishing.

## Marketing Site

The marketing site for `mite.jessica.black` lives in `site\` and is the one part
of this repository that is not Rust. It is a static [Astro](https://astro.build)
site (no UI framework runtime, no Tailwind: plain `.astro` components with scoped
styles over a `tokens.css` plus `global.css` design system). The Node toolchain
is scoped entirely to `site\`; the Rust crate at the repository root does not
depend on it and vice versa.

```powershell
cd site
npm install
npm run dev      # http://localhost:4321
npm run build    # static output to site\dist
npm run check    # astro type/diagnostics check
npm test         # vitest over the authored sample-sentence data
```

Notes for changes here:

- `site\src\data\sentences.ts` is the single source of the demo content. Every
  Japanese string is original, neutral, textbook-style example text: no game,
  visual novel, or other third-party IP, and nothing transcribed from a real
  capture. Keep it that way (this is the same constraint as Private Eval
  Disclosure, applied to site copy).
- The overlay demo, the part-of-speech color channel, and the self-defining みて
  brand term are pure CSS/markup mirrors of the product surface in
  `PRODUCT.md` and `src\win32_overlay\style.rs`. Keep the palette colorblind-safe
  and the hues confined to product-demo contexts.
- Pushes to `main` touching `site\**` deploy to GitHub Pages via
  `.github\workflows\site.yml`. `site\public\CNAME` carries the custom domain.
- `site\README.md` documents structure, fonts, and the Open Graph generator.

## Architecture Map

- `src/main.rs`: binary startup and tracing initialization only.
- `src/cli.rs`: Clap CLI dispatch and one handler per subcommand.
- `src/config.rs`: config schema and defaults. Validate boundaries here rather
  than spreading guard logic through runtime code.
- `src/capture/mod.rs`, `src/capture/window.rs`, `src/capture/image_probe.rs`,
  `src/wgc_capture.rs`: frame sources, window selection, WGC, screenshot
  fallback, and capture probe logic.
- `src/ocr.rs`, `src/ort_engine/mod.rs`, `src/ort_engine/text.rs`: OCR trait,
  ONNX Runtime engine, detector and recognizer pre/post-processing,
  TensorRT/CUDA/CPU setup, timing hooks, and OCR text normalization.
- `src/interactive/mod.rs`, `src/interactive/smoothing.rs`,
  `src/interactive/auto_capture.rs`: `watch` orchestration, worker thread,
  capture/OCR loop, smoothing handoff, the raw eval-capture hotkey, and the
  automatic eval capture (`--auto-eval-capture`) that saves a raw frame when the
  detected text or box layout changes enough to be a new scene. The fingerprint
  and change scoring live in `auto_capture.rs` as pure, unit-tested functions;
  thresholds come from the `[eval_capture]` config section.
- `src/win32_overlay/mod.rs`, `src/win32_overlay/style.rs`: layered
  click-through Win32 overlay drawing and overlay palette.
- `src/hover/mod.rs`, `src/hover/furigana.rs`, `src/hover/sense.rs`: pure hit
  testing and popup-content logic. Prefer putting
  testable geometry/text behavior here instead of inside Win32 code.
- `src/hud/mod.rs`, `src/hud/timing.rs`: latency aggregation and stage timing
  layer.
- `src/morphology.rs`, `src/frequency.rs`, `src/dictionary/mod.rs`,
  `src/dictionary/raw.rs`, `src/pos.rs`: Japanese lookup core using Lindera,
  JPDB frequency ranks, and JMdict.
- `src/eval.rs`: full-frame real-image OCR/lookup regression scoring against
  manually authored labels.
- `src/eval_capture.rs`, `src/artifact.rs`: raw eval-fixture capture, the
  on-disk `DetectionFingerprint` embedded in `capture.json` for cross-session
  auto-capture dedup, and shared on-disk artifact helpers.
- `examples/`: profiling and lookup/sense stress harnesses.
- `app/`: the Tauri desktop app (a separate, non-Rust-core surface) that
  installs, updates, and launches the CLI for non-technical users. Rust backend
  in `app/src-tauri/src/` (downloaders, window picker, watch supervisor),
  React + Vite frontend in `app/src/`. It manages a per-user "mite home" and
  spawns the CLI with that as the working directory; it does not change CLI
  behavior. The picker does not capture windows itself: it runs `mite
  list-windows --json --thumbnails`, so thumbnails come from the same WGC engine
  the watch path uses (no second capture implementation in the app), and the CLI
  drops windows that are not viable watch targets (uncapturable, timed out,
  blank, or a single solid colour) rather than returning a dead tile. See
  `app/README.md`.
- `site/`: the Astro marketing site. Its `site/src/styles/tokens.css` and
  `global.css` are the shared design source that `app/` also imports, so the app
  and site stay visually cohesive.
- `.github/workflows/release.yml`: the `v*` tag release pipeline that publishes
  `mite.exe`, the GPU runtime pack, the model manifest, the app installer,
  `release.json`, and (when the installer is updater-signed) `latest.json`. The
  same workflow also runs as a DRY RUN on every pull request and every push to
  `main` (and on manual dispatch): it builds and packages every asset but skips
  creating the GitHub Release, so the release path is a PR-time gate. main runs
  also keep the shared-key Rust/Bun caches warm for those PR dry runs. The app
  polls `release.json` to update the CLI and `latest.json` to update itself via
  `tauri-plugin-updater` (free minisign signing; Authenticode installer signing
  stays optional and unenabled). Versioning is git-tag based (build.rs `git
  describe` into `MITE_VERSION`, overridden by CI with the tag); see
  `docs/releases.md`.

## Development Rules

- This project does not need backwards compatibility by default. If the clean fix
  requires changing schemas, deleting APIs, renaming concepts, or rewriting call
  sites, do it and document the breakage plainly.
- Keep the watch-only product surface focused. The supporting commands exist to
  make `watch` usable, diagnosable, and testable.
- Do not treat a fast average as sufficient for OCR work. Tail latency is what
  makes the overlay feel good or bad.
- Keep the lookup core runnable without a live window or GPU. This is what makes
  dictionary and segmentation work fast to test.
- Prefer pure functions and unit tests for geometry, segmentation, scoring, and
  presentation decisions. Keep Win32 and GPU-specific code at the edges.
- Do not hand-roll Japanese morphology or deinflection when Lindera/JMdict data
  can express the behavior.
- For eval metadata, follow `docs/eval-metadata.md`: choose one stable,
  learner-safe primary interpretation by rule, keep visible text strict, and put
  ambiguity in notes or alternate-analysis UI rather than drifting labels by
  capture. Label corrections require pixel- or convention-level proof and an
  entry in `eval/LABEL-CHANGES.md`; `examples/audit_label_bounds.rs` and
  `examples/relabel_eval_tokens.rs` are the audited paths for them.
- Update docs when command behavior, model requirements, config defaults, or
  latency/accuracy expectations change.
- Use plain ASCII quotes in docs, code comments, and generated text.

## Verification Expectations

Run the core checks for ordinary code changes:

```powershell
cargo fmt --check
cargo test
cargo clippy --all-targets -- -D warnings
```

Also run targeted checks when relevant:

- Config changes: `cargo run -- init-config --force` in a disposable directory
  or inspect the generated TOML shape without overwriting a user's local config.
- Model/runtime changes: `cargo run -- doctor`.
- OCR, detector, recognizer, text correction, morphology, dictionary, or eval
  changes: run `cargo run -- eval --image <capture>\underlying.png --labels
  <capture>\eval.json --out target\eval\<name>.json` for any available labeled
  capture, or `.\scripts\precommit.ps1 -IncludeEval` to run every labeled
  capture in the private `eval\` submodule.
- Capture, smoothing, overlay, or timing changes: exercise `watch` against a real
  window and capture `--metrics-interval-secs` output; follow
  `docs/performance.md` for before/after evidence on latency work.
- Real eval label changes: validate the JSON by running the matching `eval`
  command. Do not crop, resize, or alter the image used by the eval.

## Cleanup And Optimization Candidates

These are not required for every change, but they are the most useful places to
invest when the touched area overlaps them.

1. Make debug and benchmark outputs consistently artifact-shaped. The repo
   already has `artifact.rs` and eval-capture JSON; examples and profiling
   harnesses could eventually emit structured reports under `target\` for easier
   comparison.
2. Keep pushing latency work through measured p95/p99 improvements. The next
   likely useful steps from `docs/future/pure-gpu.md` are INT8 recognizer
   exploration, D3D-side downscale, and avoiding full RGB materialization.
3. Consider a small smoke-test harness for CLI surfaces that do not require GPU
   assets, especially config generation, manifest validation, and window-selector
   parsing. This would catch command drift before live Windows testing.
