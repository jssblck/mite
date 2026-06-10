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
- `THIRD_PARTY_NOTICES.md` documents third-party software, models, dictionaries,
  and data that need explicit attribution or license awareness.
- `eval\AGENTS.md` documents the private eval data submodule and how to manually
  load the data-specific annotation skill when that submodule is initialized.
- `LICENSE` is the repository license. `CLAUDE.md` is intentionally only a
  compatibility pointer to this file so agent guidance does not drift.

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

First-time Windows setup:

```powershell
.\scripts\bootstrap-dev.ps1
.\scripts\bootstrap-dev.ps1 -IncludeEvalData
.\scripts\bootstrap-dev.ps1 -SkipGpuRuntime
```

Product commands:

```powershell
cargo run -- list-windows
cargo run -- watch
cargo run -- watch --title "Window title" --auto
cargo run -- watch --hud
cargo run -- watch --metrics-interval-secs 5
cargo run -- eval --image eval\collection-name\capture-<ts>\underlying.png --labels eval\collection-name\capture-<ts>\eval.json --out target\eval\capture-<ts>.json
cargo run -- eval-corpus --root eval --out target\eval\corpus-summary.json --out-dir target\eval\corpus --allow-failures
```

Use `eval` against manually labeled real captures as the OCR plus dictionary
regression guard. Use `watch
--metrics-interval-secs` or `--hud` as the latency acceptance loop. A full 4K pass
on the TensorRT path should stay near the documented ~100 ms p95 on the reference
NVIDIA setup; watch p95/p99, not just averages.

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
TensorRT/CUDA/cuDNN runtime DLLs are cached with
`scripts\bootstrap-dev.ps1 -GpuRuntimeOnly` and staged by `build.rs`.
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

## Architecture Map

- `src/main.rs`: CLI dispatch. Keep it thin; one handler per subcommand.
- `src/config.rs`: config schema and defaults. Validate boundaries here rather
  than spreading guard logic through runtime code.
- `src/capture/mod.rs`, `src/capture/window.rs`, `src/capture/image_probe.rs`,
  `src/wgc_capture.rs`: frame sources, window selection, WGC, screenshot
  fallback, and capture probe logic.
- `src/ocr.rs`, `src/ort_engine/mod.rs`, `src/ort_engine/text.rs`: OCR trait,
  ONNX Runtime engine, detector and recognizer pre/post-processing,
  TensorRT/CUDA/CPU setup, timing hooks, and OCR text normalization.
- `src/interactive/mod.rs`, `src/interactive/smoothing.rs`: `watch`
  orchestration, worker thread, capture/OCR loop, smoothing handoff, debug
  capture trigger.
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
- `src/debug_capture.rs`, `src/artifact.rs`: self-contained diagnostic output.
- `examples/`: profiling and lookup/sense stress harnesses.

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
   already has `artifact.rs` and debug-capture JSON; examples and profiling
   harnesses could eventually emit structured reports under `target\` for easier
   comparison.
2. Keep pushing latency work through measured p95/p99 improvements. The next
   likely useful steps from `docs/future/pure-gpu.md` are INT8 recognizer
   exploration, D3D-side downscale, and avoiding full RGB materialization.
3. Consider a small smoke-test harness for CLI surfaces that do not require GPU
   assets, especially config generation, manifest validation, and window-selector
   parsing. This would catch command drift before live Windows testing.
