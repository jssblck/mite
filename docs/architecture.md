# Architecture

Mite is the `watch` overlay plus the offline lookup core that backs it. The live
path is: capture a window -> detect + recognize text on the GPU -> group adjacent
recognized lines into text blocks -> segment those blocks into dictionary words ->
project the words back to visible line spans -> draw a translucent,
click-through overlay you can hover to define. The vision path is built around
replaceable boundaries so the low-latency policy can be tested before
hardware-specific code is finished.

## Boundaries

- `FrameSource` (`capture/mod.rs`, `capture/window.rs`,
  `capture/image_probe.rs`, `wgc_capture.rs`): produces frames. The window
  implementations are `WindowsGraphicsCapture` (first-party WGC, for
  fullscreen-windowed DirectX/game windows), `WindowScreenshotCapture` (the
  legacy xcap path), and `AutoWindowCapture` (the default: it starts a persistent
  WGC session, keeps it when a probe frame is meaningful, and falls back to xcap
  only when WGC cannot produce a usable frame). `ImageFileCapture` feeds the
  offline eval. Select a window by title substring / window id / pid via
  `WindowSelector` (fields are private; the all-empty, matches-nothing state is
  unconstructable, so "≥1 criterion" holds by construction). The WGC path still
  uses CPU readback; production low-latency work should move toward
  device-resident handoff (see `docs/future/pure-gpu.md`).
- `OcrEngine` (`ocr.rs`): performs detection and recognition. `OrtOcrEngine`
  (`ort_engine/mod.rs`, `ort_engine/text.rs`) runs the PP-OCRv5 ONNX
  detector/recognizer and OCR text normalization. The default pass recognizes
  every plausible detector box and keeps short Japanese lines if they clear the
  confidence floor, since game UI often uses compact labels. An optional
  `detector_low_contrast_pass` runs a second detector inference over a
  local-contrast luminance image, then dedupes those boxes against the primary
  pass; it is off by default because it spends extra GPU time only for frames
  where the primary detector truly misses faint text. `MockOcrEngine` backs unit
  tests. `StableIdAllocator` assigns cross-frame-stable box ids so a box that
  stays put isn't treated as new each frame.
- `interactive::Worker` (`interactive/mod.rs`, `interactive/smoothing.rs`):
  owns the engine, dictionary, capture source, and `SmoothingState`, and runs one
  capture -> detect -> recognize ->
  block-analyze pass per requested window id on a background thread. Text-block
  analysis (`text_blocks.rs`) keeps detector boxes intact. Geometry first
  nominates adjacent same-column lines; dictionary analysis joins a pair only
  when a known token crosses the line boundary and at least one boundary side is
  a fragment rather than a complete known token. Wrapped words such as
  `クリティカルダ` + `メージ` therefore resolve as one `ダメージ` token, while
  stacked stat rows stay line-local. Each block token is then projected back
  onto the line segment where it is visible, so hover geometry still follows the
  detector boxes. The same worker also handles the opt-in raw eval-capture
  hotkey by stopping after `FrameSource::next_frame` and writing the
  PNG/metadata without OCR. The UI thread (overlay message pump, cursor polling,
  hover → popup) stays responsive at ~60 Hz and applies snapshots as the worker
  produces them, so a slow OCR pass never freezes hovering.
- `Win32Overlay` (`win32_overlay/mod.rs`, `win32_overlay/style.rs`): the
  presentation layer - a per-pixel-alpha layered topmost window
  (`UpdateLayeredWindow` over a 32-bit BGRA buffer) that draws POS-coloured word
  highlights, the furigana definition popup, and the latency HUD. The pure,
  Win32-free hit-testing and popup-content logic lives in `hover/mod.rs`,
  `hover/furigana.rs`, and `hover/sense.rs` so it can be unit-tested off-Windows.

### Temporal smoothing (reuse policy)

`SmoothingState` reuses the previous full detection while the *text regions* are
unchanged, skipping detect + recognize + analyze. It samples luma at the
previously detected text rects (an `Anchor`) rather than hashing the whole frame,
so an animated game background outside the text doesn't force a re-OCR; the reuse
is bounded by a max age (default 3 s) so any missed change self-corrects. This is
the only frame-reuse mechanism — `watch` does not run through a generic pipeline
scheduler.

## Real OCR runtime

The default runtime config is `nvidia_tensor_rt_then_cuda`, and the GPU path is
wired end to end with a graceful fallback chain **TensorRT -> CUDA -> CPU**
(`commit_session` in `ort_engine/mod.rs`):

1. **TensorRT EP** (FP16) builds one engine per model from a single dynamic-shape
   optimization profile, so the variable-width recognizer and variable-size
   detector are each served by *one* cached engine - no per-shape rebuild. Engines
   and the kernel timing cache persist under `cache/engines`; the multi-minute
   first build is paid once. CUDA is co-registered so any op TensorRT declines
   runs on the GPU rather than the CPU.
2. **CUDA EP** is the fallback, tuned for the dynamic-shape pipeline (`tuned_cuda_ep`):
   `Heuristic` cuDNN conv-algorithm search (the default `Exhaustive` re-benchmarks
   every unseen input width — tens of ms per call) and TF32 tensor-core math.
3. **CPU EP** is the last resort for correctness without a GPU runtime.

TensorRT and CUDA need NVIDIA runtime DLLs next to the binary; ORT ships only the
provider shims. Run `scripts\bootstrap-dev.ps1 -GpuRuntimeOnly` once to fetch
pinned TensorRT 10 / CUDA 12 / cuDNN 9 wheels into `.gpu-runtime\bin`. `build.rs`
stages that cache into the active Cargo profile output dir on every build, so
debug and release binaries get the same provider DLL dependencies.

### Pipeline latency (reference NVIDIA GPU, true 4K / 3840x2160 game window)

Per-stage p95 of a full OCR pass, measured via `watch --metrics-interval-secs`:

| stage     | CPU-resize + fp32 CUDA (before) | low-res TensorRT fp16 | native 4K TensorRT fp16 |
|-----------|--------------------------------:|----------------------:|------------------------:|
| capture   |  16 ms |  17 ms |  17 ms |
| detect    | 246 ms |  28 ms | measured per scene |
| recognize | 212 ms |  52 ms | measured per scene |
| present   |   8 ms |   9 ms |   9 ms |
| **total** | **~464 ms** | **~100 ms** | **higher recall, higher latency** |

Two changes drive it. (1) **Detector preprocessing moved off the single-threaded
bicubic path**. The default native-4K path pads to the detector shape instead of
resampling the full frame, and the optional low-resolution path uses a
rayon-parallel coverage-weighted area resampler (`area_downscale`): the old
4K-to-1920 shrink fell from ~208 ms to ~8 ms, and area averaging is higher
quality than bicubic for downscaling text (it integrates all source detail and
can't ring/alias). The recognizer still crops from the full-res frame, so
recognition quality is independent of the detector downscale. (2)
**TensorRT FP16** fuses the mobile models' many depthwise convolutions, which on
the CUDA EP were launch-bound (~33 ms of fixed kernel-launch overhead per
recognizer call regardless of data); the per-batch recognizer inference dropped
from ~33 ms to ~4-9 ms and is accuracy-neutral versus fp32.

The TensorRT detector profile accepts inputs up to a 3840 px long side. The
default 4K watch path detects at native 3840 for recall on dense game-menu
labels. The old low-resolution fast path is still available by setting
`pipeline.detector_downscale = 0.5`, `pipeline.detector_min_long_side = 1920`,
and `pipeline.detector_max_long_side = 1920`.

Detector preprocessing pads the resized content to PP-OCR's multiple-of-32 input
shape instead of resampling solely for shape alignment. This matters most at
native 4K: a 3840x2160 frame becomes a 3840x2176 tensor without touching the
3840x2160 content pixels, and postprocessing maps boxes through the unpadded
content extent so the padding cannot shift or create user-visible boxes.
Tiny or unusually shaped inputs still resize to the stride-aligned shape when
padding would otherwise be a material fraction of the detector tensor.

#### Mobile vs. server models

The PP-OCRv5 **mobile** models are the default: with TensorRT they are fast
(~100 ms/pass) and accuracy-neutral. The **server** models read body text more
accurately (they fix e.g. 自身/終奏スキル misreads) but are far heavier, and they
**overflow in FP16** (garbage output), so they require FP32, which is slower and
uses more GPU. They are an opt-in accuracy/latency trade: point
`models.detector_path`/`recognizer_path` at the `*-server-*.onnx` files and set
`runtime.fp16 = false`.

## Latency acceptance

Use `cargo run -- watch --metrics-interval-secs N` (or `--hud`) against a live
window as the acceptance loop; it reports rolling p50/p95/p99 for each stage
(capture, detect, recognize, analyze, present). With the TensorRT path a full 4K
pass is ~100 ms p95 (well under the original 500 ms target); guard against
regressions toward the old ~460 ms. A fast average is not enough - large p95
spikes mean the overlay will feel bad even if the mean looks fine.
