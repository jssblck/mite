# Mite

Mite is a Windows-first, local, low-latency OCR overlay for Japanese text in games and visual novels - a yomichan-style point-and-define experience for anything on screen. It captures a target window with Windows Graphics Capture, runs PaddleOCR PP-OCRv5 detector + recognizer ONNX models on an NVIDIA GPU, segments the recognized text into words, looks each up in a bundled JMdict dictionary, and draws a translucent, click-through, topmost overlay you can hover to get definitions.

The single command that does all of this is **`watch`**. The rest of the CLI exists only to support it: find a window to capture (`list-windows`), score a manually labeled real capture (`eval`), check the GPU/model setup (`doctor`), and write a config (`init-config`).

On the reference NVIDIA setup, a full OCR pass over a true-4K game window is **~100 ms p95**, via a **TensorRT -> CUDA -> CPU** fallback chain: TensorRT (FP16, one cached engine per model from a dynamic-shape profile) is the fast path, the CUDA EP (tuned cuDNN heuristic search + TF32) is the fallback, and CPU is the last resort. See `docs/architecture.md` for the per-stage breakdown and the mobile-vs-server model trade.

## Quick start

```powershell
.\scripts\setup-windows.ps1      # local dirs + Rust/nvidia-smi checks
.\scripts\download-models.ps1    # PP-OCRv5 ONNX + JMdict + JPDB freq list -> models/
cargo build --release
.\scripts\install-gpu-runtime.ps1 # fetch TensorRT/CUDA/cuDNN DLLs into .gpu-runtime and stage the build
cargo run -- doctor              # confirm GPU + model files are ready
cargo run -- list-windows        # find the window id / title to capture

cargo run -- watch                                   # hold SHIFT over a window to OCR + define
cargo run -- watch --title "Target Game" --auto      # 3D game that swallows Shift: pin it, run continuously
cargo run -- watch --hud                             # with the on-screen latency graph
cargo run -- watch --metrics-interval-secs 5         # headless per-stage timing to stderr
```

`mite.toml` is optional — without it, built-in Windows/NVIDIA defaults are used (`cargo run -- init-config` writes a template you can edit). The defaults assume:

- runtime backend: `nvidia_tensor_rt_then_cuda`, FP16 on
- detector model: `models/pp-ocrv5-mobile-det.onnx`, recognizer: `models/pp-ocrv5-mobile-rec.onnx`
- detector input long side: native resolution up to 4K (`detector_downscale` `1.0`, `detector_min_long_side` `3840`, `detector_max_long_side` `3840`), resized only above the cap, then padded to the detector's multiple-of-32 input shape
- detector postprocessing: probability threshold `0.30`, component score threshold `0.45`, max text-box height ratio `0.08`, max text-box area ratio `0.02`
- recognition presentation filter: minimum confidence `0.50`, or `0.85` for single-character results; short Japanese lines are kept when they clear the confidence floor
- optional recall knob: `detector_low_contrast_pass = true` runs a second detector pass on a local-contrast view, trading extra detector latency/GPU time for faint text that the standard pass misses
- optional low-resolution detector mode: set `detector_downscale = 0.5`, `detector_min_long_side = 1920`, and `detector_max_long_side = 1920` to trade small-label recall for lower detector latency/GPU time on 4K windows

## Local Windows setup

```powershell
.\scripts\setup-windows.ps1
```

creates the local model/cache directories and checks Rust and `nvidia-smi`.

### TensorRT acceleration (recommended)

ONNX Runtime ships the TensorRT/CUDA *provider shims* but not the NVIDIA runtime DLLs they depend on. Fetch the pinned TensorRT 10 / CUDA 12 / cuDNN 9 runtime once:

```powershell
.\scripts\install-gpu-runtime.ps1
cargo build --release
```

This uses `uv pip install` to download pinned NVIDIA wheels into `.venv-models`, copies the redistributable DLLs into `.gpu-runtime\bin`, and stages them into any existing `target\debug` / `target\release` outputs. `build.rs` stages the same cache automatically for future Cargo builds, including debug builds, release builds, `deps`, and examples. The first `watch` run builds and caches the FP16 engines under `cache/engines` (a one-time, multi-minute, GPU-heavy build); subsequent runs load them instantly. Without the DLL cache, mite still works by falling back from TensorRT to CUDA or CPU, but `doctor` and the provider warnings will tell you exactly which DLLs are missing. Confirm with the `TensorRT execution provider active` log line.

**Mobile vs. server models (accuracy/latency knob):** the default mobile models are fast and accurate under TensorRT. For higher body-text accuracy at the cost of speed/GPU, download the server variants and point the config at them with FP16 disabled (server models overflow in FP16):

```powershell
# optional, larger + heavier; requires runtime.fp16 = false in mite.toml
$base = "https://www.modelscope.cn/models/RapidAI/RapidOCR/resolve/v3.8.0/onnx/PP-OCRv5"
curl.exe -L "$base/det/ch_PP-OCRv5_det_server.onnx" -o models\pp-ocrv5-server-det.onnx
curl.exe -L "$base/rec/ch_PP-OCRv5_rec_server.onnx" -o models\pp-ocrv5-server-rec.onnx
```

### Models and dictionaries

```powershell
.\scripts\download-models.ps1
```

installs the PP-OCRv5 ONNX models, the JMdict English lexicon (`models/jmdict-eng.json`, JMdict/EDICT data via scriptin/jmdict-simplified, CC BY-SA 4.0), and the JPDB frequency list (`models/jpdb-freq/`, rank-based frequencies from [MarvNC/jpdb-freq-list](https://github.com/MarvNC/jpdb-freq-list), used to weight segmentation). All assets are recorded in `models/MODELS.lock.json`.

See `docs/local-windows.md` for the CUDA/cuDNN runtime requirements and troubleshooting.

## The `watch` overlay

`watch` runs as a persistent background process and draws a transparent, click-through, topmost overlay over whatever you're looking at:

- **Hold `Shift`** to OCR the current foreground window. Each recognized **word** (from the lattice segmentation) is drawn as a mostly-transparent fill, colour-coded by part of speech - particles amber, nouns blue, verbs green, adjectives orange, etc. While `Shift` stays held the window is re-OCR'd on a throttle (default every 600 ms) so changing dialogue keeps up.
- **Hover** a word to look it up: its highlight brightens and a polished definition popup appears with **furigana aligned over the kanji** (okurigana matching, so 取り出す shows 取[と]り出[だ]す), an inflection note, and the glosses. Word boundaries come from the segmentation, so hovering resolves the exact word under the cursor.
- **Sticky for review:** if the cursor is over a highlighted word or the popup when you release `Shift`, the overlay stays open (and OCR freezes) so you can read or screenshot it. Move away and it clears.
- **Problem report button:** the popup has a camera button in its top-right corner. Clicking it writes a self-contained **debug capture** to `%LOCALAPPDATA%\mite\debug-captures\capture-<ts>\` — the OCR'd window frame (`underlying.png`), that frame with the overlay composited on top (`with_overlay.png`), and `capture.json` with the window id, screen rect, raw OCR lines (text + confidence + per-glyph centres), and every word's surface / dictionary form / furigana / glosses / category / rect. Intended for filing concrete issues seen in games.
- **Raw eval capture hotkey:** `--enable-eval-hotkey COMBO` registers a global hotkey that captures the current target window through the active Mite capture backend and writes only the raw frame, with no OCR. Captures go to `%LOCALAPPDATA%\mite\eval-captures\capture-<ts>\` by default, or `--eval-capture-dir DIR`.
- **Release `Shift`** (cursor away from content) to clear the overlay; press **`Esc`** to quit.
- **Games that swallow `Shift`:** many games consume the `Shift` key while focused, so the hold-to-activate trigger never fires. Use **`--auto`** to run the capture/OCR loop continuously with no key held; pin the target window with **`--window-id`** (from `list-windows`) or **`--title`** since the game stays foreground. `--capture-backend wgc` forces Windows Graphics Capture (the path that works on 3D/DirectX windows; the desktop-screenshot fallback can return wrong/black frames for games).

```powershell
cargo run -- watch
cargo run -- watch --refresh-ms 400 --max-senses 2 --max-glosses 3
cargo run -- watch --title "Target Game" --auto             # pin a 3D game, run continuously
cargo run -- watch --hud                                    # on-screen per-stage latency graph
cargo run -- watch --metrics-interval-secs 5                # aggregated p50/p95/p99 to stderr
cargo run -- watch --title "Target Game" --enable-eval-hotkey Ctrl+Alt+F12
```

The overlay stays click-through, so the target game keeps all keyboard/mouse input — the cursor is read by polling rather than by capturing mouse events, and the foreground window is captured via a WGC-then-screenshot path. It's a per-pixel-alpha layered window (`UpdateLayeredWindow` over a 32-bit BGRA buffer), which is what lets highlights be genuinely translucent while the popup stays opaque. Capture and OCR run on a background worker thread so the overlay and hover stay responsive at ~60 Hz while a pass is in flight. The process is set per-monitor DPI aware so cursor coordinates line up with captured pixels. To keep latency low on animated 3D backgrounds, a full pass is reused (detection skipped) while the *text regions* are unchanged — sampled by luma at the previously detected rects — and re-run at least every few seconds so any missed change self-corrects.

Each OCR pass logs the target window, the backend used, the frame size, and the line count; `--metrics-interval-secs N` (or `--hud`) reports rolling p50/p95/p99 for each stage (capture, detect, recognize, analyze, present) — the latency feedback loop for keeping the overlay smooth.

## How the Japanese lookup works

Recognized text is segmented and resolved by an offline lookup core (no GPU or live window needed - it is exercised by the real-image `eval` command and unit tests):

- **Segmentation + lemmatization** use [Lindera](https://github.com/lindera/lindera) (a pure-Rust MeCab-compatible analyzer with embedded IPADIC), so conjugation/inflection is handled natively (食べました → 食べる, 高かった → 高い) rather than by hand-rolled rules.
- **Wrapped text blocks** are analyzed as blocks, not isolated detector lines. Geometry nominates adjacent same-column OCR lines, then dictionary analysis joins them only when a known token crosses the boundary and at least one side is a fragment rather than a complete known token. That lets a word split by UI wrapping, such as `クリティカルダ` + `メージ`, hover and score as `ダメージ` while independent stat rows such as `HP`, `攻撃力`, and `クリティカル` stay line-local.
- **Word boundaries** come from a **minimum-cost-path (Viterbi) lattice over JMdict terms**: each single morpheme is a node, as is every adjacent span whose fused form is a JMdict entry, with node cost `ln(frequency rank)` plus a small per-token penalty. Frequencies come from the **JPDB** list (scraped from an anime/drama/light-novel/visual-novel/game corpus, so it ranks domain vocabulary well). Real compounds fuse because they're cheaper as one node than as rarer-summed pieces (聖遺物, 必殺技, 攻撃力, 個体値), while grammatical coincidences stay split because the function morphemes are so frequent that splitting wins. No hand-tuned gate — frequency does the disambiguation. (If the JPDB list is absent, segmentation still works but degrades to a fewest-tokens preference.)
- **Definitions** come from the bundled JMdict lexicon (glosses + parts of speech); the entry whose POS agrees with Lindera's analysis is surfaced first, so homographs resolve to their grammatical sense (は as the topic particle, not the noun 羽).

## Accuracy regression guard (`eval`)

The real capture corpus lives in the private `eval/` submodule. If `eval/` is
empty after cloning Mite, initialize the submodule and pull its Git LFS objects:

```powershell
git submodule update --init eval
git -C eval lfs pull
```

`eval` runs the whole image -> OCR -> segment -> lookup -> popup-metadata path
over one full-frame real capture and scores it against a manually authored
`eval.json`. Mite does not generate labels. The label file is expected to contain
precise detection bounds, exact per-character text labels and bounds, per-character
token ids, and expected token metadata such as part of speech, dictionary form,
furigana, note, and glosses.

```powershell
cargo run -- eval `
  --image eval\collection-name\capture-<ts>\underlying.png `
  --labels eval\collection-name\capture-<ts>\eval.json `
  --out target\eval\capture-<ts>.json
```

The command reports an aggregate score plus separate detection, character, and
metadata scores. Unexpected OCR detections reduce the score unless `eval.json`
explicitly marks that text or region as ignored. The command exits non-zero
unless the run is perfect; pass `--allow-failures` when you want a report while
hillclimbing accuracy.

Detection bounds are scored with jitter tolerance. By default, x/y drift up to
`max(4px, 0.20 * expected_height)` and width/height drift up to
`max(6px, 0.30 * expected_height)` get full detection credit; larger drift
decays to zero at three times that tolerance. Individual labels may override this
with `bounds_tolerance`. Character text and metadata are still exact.

### Eval label UI

`eval-ui` is a separate developer binary for reviewing and authoring
`eval/` labels in a local browser UI:

```powershell
cargo run --bin eval-ui
```

Open the printed `http://127.0.0.1:8765/` URL. The UI lists eval folder bundles
under `eval/` (for example `collection-name\capture-<ts>\`), opens each bundle's
fixed `underlying.png` or `underlying.jpg`, loads sibling `eval.json` files,
draws label/ignored/raw/diff overlays on the full image, lets you drag or resize
label boxes, and saves validated label JSON back into the bundle. New labels can
be drawn by hand, or adopted from raw Mite OCR detections. When text or bounds
change, the UI rebuilds per-character bounds and token metadata from the same
JMdict/Lindera lookup path used by `watch` and `eval`.

Useful development flags:

```powershell
cargo run --bin eval-ui -- --eval-root eval\collection-name
cargo run --bin eval-ui -- --mock-ocr
cargo run --bin eval-ui -- --port 0
```

`--mock-ocr` is meant for UI smoke tests without loading ONNX Runtime. Use the
default runtime when reviewing true raw detections and diffs.

## Commands

```powershell
cargo run -- init-config [--force]   # write a default mite.toml
cargo run -- doctor                  # nvidia-smi probe + model-file validation
cargo run -- list-windows            # id | pid | geometry | title for every capturable window
cargo run -- eval --image path\to\underlying.png --labels path\to\eval.json [--out path.json] [--allow-failures]
cargo run --bin eval-ui              # browser UI for eval label review/authoring
cargo run -- clean-images [--dry-run] # remove local debug/eval capture images under %LOCALAPPDATA%\mite
cargo run -- watch [--title T | --window-id N | --pid P] [--auto] [--hud] [--metrics-interval-secs N] [--refresh-ms N] [--no-smoothing] [--enable-eval-hotkey COMBO]
```

## Testing

```powershell
cargo fmt --check
cargo test
cargo clippy --all-targets -- -D warnings
```

Install the local precommit hook once if you want these checks before every
commit:

```powershell
.\scripts\install-precommit-hook.ps1
```

The hook runs `.\scripts\precommit.ps1`. Use
`.\scripts\precommit.ps1 -IncludeEval` when OCR/lookup accuracy should be part of
the local gate.

## License

Mite is licensed under the GNU Affero General Public License v3.0. See
`LICENSE`.

Runtime OCR models, dictionary data, frequency data, ONNX Runtime components,
NVIDIA runtime DLLs, and other third-party materials remain under their own
upstream terms. See `THIRD_PARTY_NOTICES.md` and `model-manifest.json` for the
main notices and attribution records.
