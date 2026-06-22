# Mite

Mite is a Windows-first OCR overlay for reading Japanese text in games, visual
novels, manga readers, videos, and other visual content. It is aimed at English
speakers learning Japanese who want a Yomichan-style point-and-define workflow
outside the browser.

Mite captures a target window, runs PP-OCRv5 detector and recognizer ONNX models
through ONNX Runtime, segments recognized Japanese with Lindera and JMdict, then
draws a transparent click-through Win32 overlay with hover definitions and
furigana. The primary command is `watch`; the rest of the CLI supports setup,
diagnostics, evals, and developer tooling.

> [!NOTE]
> Mite is focused on native Windows apps, games, and visual media. For ordinary
> webpages, browser-native tools such as Yomitan, Yomichan, or 10ten are usually
> the better fit.

## Install

Most people should use the Mite desktop app. Download the installer from the
[releases page](https://github.com/jssblck/mite/releases) and run it: the app
walks you through a one-time setup (it installs the engine, downloads the
recognition models, and, on NVIDIA GPUs, an optional acceleration pack), then
lets you pick a window from a live preview grid and start reading. No terminal
required, and it keeps the engine up to date for you. See
[app/README.md](app/README.md) for what the app manages.

The rest of this document covers building from source, the path for developers
and contributors.

## How well does it read?

Measured against several hundred hand-checked 4K game screenshots, about 94 of
every 100 text lines are read perfectly, and roughly 1 character in 80 is wrong
or missing. Mistakes concentrate in tiny, faint, or heavily stylized text;
ordinary dialogue and menus are read very reliably. When a misread does happen,
it usually shows up as an "unknown" word rather than a wrong definition. What
the mistakes look like in practice, and how much to trust a popup, is covered
in [docs/accuracy.md](docs/accuracy.md).

On the reference NVIDIA setup, a fresh full-screen read of a 4K game frame
takes about a fifth of a second, so definitions feel immediate.

## Status

Mite is local-first and optimized for Windows/NVIDIA systems. The reference path
uses a TensorRT -> CUDA -> CPU fallback chain and targets low-latency 4K OCR. The
lookup core and eval tooling are designed to remain testable without a live game
window.

Important details:

- Rust 2024 project using `cargo`.
- Windows Graphics Capture is the preferred capture backend for games.
- Default OCR assets are PP-OCRv5 mobile detector/recognizer ONNX files.
- Runtime model, dictionary, frequency, GPU DLL, cache, and eval data files are
  not committed to the source repository.
- Real-image eval data lives in the private `eval/` submodule.

## Quick start (build from source)

Run the consolidated developer setup script from PowerShell:

```powershell
.\scripts\bootstrap-dev.ps1
```

The script checks for Git, Rust, `uv`, and an NVIDIA driver; downloads OCR
models, JMdict, and JPDB frequency data; installs local Git hooks; creates
`mite.toml` when missing; installs the pinned TensorRT/CUDA/cuDNN runtime cache;
builds Mite; and runs `doctor`.

Useful setup modes:

```powershell
.\scripts\bootstrap-dev.ps1 -ModelsOnly
.\scripts\bootstrap-dev.ps1 -ModelsOnly -IncludeServerModels
.\scripts\bootstrap-dev.ps1 -GpuRuntimeOnly
.\scripts\bootstrap-dev.ps1 -HooksOnly
.\scripts\bootstrap-dev.ps1 -EvalDataOnly
.\scripts\bootstrap-dev.ps1 -SkipGpuRuntime
```

Then find a target window and start the overlay:

```powershell
cargo run -- list-windows
cargo run -- watch
cargo run -- watch --title "Target Game" --auto
cargo run -- watch --hud
cargo run -- watch --metrics-interval-secs 5
```

Use `--auto` for games that consume the `Shift` key, and pin the target with
`--title`, `--window-id`, or `--pid`.

## Features

- Window OCR overlay for Japanese text in native Windows apps and games.
- Hover popups with dictionary forms, glosses, inflection notes, and furigana.
- Click-through layered Win32 overlay that keeps game input uninterrupted.
- TensorRT/CUDA acceleration with CPU fallback.
- Temporal smoothing so stable text regions can be reused instead of re-OCR'd
  every frame.
- Raw debug captures for concrete issue reports.
- Manual real-image eval workflow for OCR, lookup, bounds, and popup metadata.
- Browser-based eval label UI for private eval corpora.

## Commands

```powershell
cargo run -- init-config [--force]
cargo run -- doctor
cargo run -- list-windows
cargo run -- watch [--title T | --window-id N | --pid P] [--auto] [--hud]
cargo run -- eval --image path\to\underlying.png --labels path\to\eval.json
cargo run -- eval-corpus --root eval --out target\eval\corpus-summary.json --allow-failures
cargo run --bin eval-ui
cargo run -- clean-images [--dry-run]
```

## Documentation

- [Local Windows usage](docs/local-windows.md): setup, running the overlay,
  capture troubleshooting.
- [Character accuracy](docs/accuracy.md): how accurate the OCR is, why, and
  where the limits are. Start here if you want to know whether to trust a popup.
- [Architecture](docs/architecture.md): runtime boundaries, GPU pipeline, and
  latency.
- [Model setup and provenance](docs/models.md): the OCR models and their
  trade-offs.
- [Performance evidence guide](docs/performance.md): how latency claims are
  measured, with current reference numbers.
- [Eval metadata policy](docs/eval-metadata.md): which dictionary
  interpretation mite teaches when several are valid.
- [Pure-GPU exploration notes](docs/future/pure-gpu.md): exploratory, not
  scheduled.
- [Third-party notices](THIRD_PARTY_NOTICES.md) and
  [model manifest](model-manifest.json)
- [Agent guidance](AGENTS.md)

## Development

Core checks:

```powershell
cargo fmt --check
cargo test
cargo clippy --all-targets -- -D warnings
```

The local Git hook runs `.\scripts\precommit.ps1`. Install or refresh it with:

```powershell
.\scripts\bootstrap-dev.ps1 -HooksOnly
```

Run private real-image evals when OCR, dictionary, detection, recognition, eval,
or popup metadata behavior changes:

```powershell
.\scripts\bootstrap-dev.ps1 -EvalDataOnly
cargo run -- eval-corpus --root eval --out target\eval\corpus-summary.json --allow-failures
.\scripts\precommit.ps1 -IncludeEval
```

The private eval submodule contains corpus-specific annotation instructions and
the eval annotation skill under `eval\.agents\`.

## Runtime Assets And Data

The following paths are local artifacts and intentionally ignored:

- `models\`
- `cache\`
- `target\`
- `mite.toml`
- `.gpu-runtime\`
- `.venv-models\`
- `.env`

OCR models, JMdict, JPDB frequency data, NVIDIA runtime DLLs, ONNX Runtime
components, and eval captures remain under their own upstream terms. See
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) and
[model-manifest.json](model-manifest.json) before redistributing any runtime
assets or generated bundles.

## License

Mite is licensed under the GNU Affero General Public License v3.0. See
[LICENSE](LICENSE).
