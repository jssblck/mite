# Architecture

In plain terms: mite takes a picture of your game window many times a second,
finds the Japanese text in it, reads the text, looks the words up in a
dictionary, and draws definitions over the game. Everything runs on your own
PC; no image or text leaves the machine. Fresh text is read and defined in
roughly a fifth of a second on the reference GPU. The rest of this page is for
people working on the code.

Mite is the `watch` overlay plus the offline lookup core that backs it. The
live path is: capture a window -> detect + recognize text on the GPU -> group
adjacent recognized lines into text blocks -> segment those blocks into
dictionary words -> project the words back to visible line spans -> draw a
translucent, click-through overlay you can hover to define. The vision path is
built around replaceable boundaries so policy can be tested without live
hardware.

## Boundaries

- `FrameSource` (`capture/mod.rs`, `capture/window.rs`,
  `capture/image_probe.rs`, `wgc_capture.rs`): produces frames. The window
  implementations are `WindowsGraphicsCapture` (first-party WGC, for
  fullscreen-windowed DirectX/game windows), `WindowScreenshotCapture` (the
  legacy xcap path), and `AutoWindowCapture` (the default: it starts a
  persistent WGC session, keeps it when a probe frame is meaningful, and falls
  back to xcap only when WGC cannot produce a usable frame). `ImageFileCapture`
  feeds the offline eval. Select a window by title substring / window id / pid
  via `WindowSelector` (fields are private; the all-empty, matches-nothing
  state is unconstructable, so ">= 1 criterion" holds by construction). Frame
  pixels are shared (`Arc<RgbImage>`), so retaining a frame across passes (WGC
  stale-serve, smoothing anchors, snapshots) never copies the ~25 MB 4K
  buffer. A `FrameSource` can also be handed a `FrameProbe` - the smoothing
  anchor's luma signature - and the WGC implementation evaluates it directly
  on the mapped staging buffer: when the sampled text regions are unchanged it
  answers without converting, fingerprinting, or allocating a frame at all,
  which removes the whole BGRA->RGB materialization cost from stable-scene
  passes. The WGC path still uses CPU readback for changed frames;
  device-resident handoff is exploratory (see `docs/future/pure-gpu.md`).
- `OcrEngine` (`ocr.rs`): performs detection and recognition. `OrtOcrEngine`
  (`ort_engine/mod.rs`, `ort_engine/text.rs`) runs the PP-OCRv5 ONNX
  detector/recognizer plus OCR text normalization. Detection runs twice per
  frame by default: a standard pass and a `detector_low_contrast_pass` over a
  local-contrast luminance image, deduped against the primary boxes; the
  second pass recovers faint text on translucent game panels. The dual-pass
  schedule keeps the GPU saturated: while the primary inference holds the
  session, a worker thread builds the local-contrast image (a parallel
  sliding-window box mean, exact-equal to the integral-image formulation) and
  its NCHW tensor, so the second inference launches the moment the first
  returns; the primary probability map's postprocessing runs concurrently
  with that second inference, handed across threads as the owned ORT output
  value (no copy). Detector postprocessing extracts components with scanline
  union-find (bit-identical to the old per-pixel flood fill, several times
  faster). Recognition runs width-sorted batches through a pack -> infer ->
  decode thread pipeline (identical batch composition to a sequential loop),
  so tensor packing and parallel CTC decode overlap GPU inference instead of
  serializing with it. Box candidates are capped per frame by confidence rank, never by
  screen position, so dense menus cannot silently lose a region. Recognition
  keeps short Japanese lines that clear the confidence floor (game UI uses
  compact labels), synthesizes the diamond bullets that PP-OCR's character set
  lacks via a gated geometric classifier, and can consult an optional heavier
  fallback recognizer on low-confidence lines (`docs/models.md`).
  `MockOcrEngine` backs unit tests. `StableIdAllocator` assigns
  cross-frame-stable box ids so a box that stays put is not treated as new
  each frame.
- `interactive::Worker` (`interactive/mod.rs`, `interactive/smoothing.rs`):
  owns the engine, dictionary, capture source, and `SmoothingState`, and runs
  one capture -> detect -> recognize -> block-analyze pass per requested
  window id on a background thread. Text-block analysis (`text_blocks.rs`)
  keeps detector boxes intact. Geometry first nominates adjacent same-column
  lines; dictionary analysis joins a pair only when a known token crosses the
  line boundary and at least one boundary side is a fragment rather than a
  complete known token. Wrapped words such as `クリティカルダ` + `メージ`
  therefore resolve as one `ダメージ` token, while stacked stat rows stay
  line-local. Each block token is then projected back onto the line segment
  where it is visible, so hover geometry still follows the detector boxes. The
  same worker handles the opt-in raw eval-capture hotkey by stopping after
  `FrameSource::next_frame` and writing the PNG/metadata without OCR. The UI
  thread (overlay message pump, cursor polling, hover -> popup) stays
  responsive at ~60 Hz and applies snapshots as the worker produces them, so a
  slow OCR pass never freezes hovering.
- `Win32Overlay` (`win32_overlay/mod.rs`, `win32_overlay/style.rs`): the
  presentation layer - a per-pixel-alpha layered topmost window
  (`UpdateLayeredWindow` over a 32-bit BGRA buffer) that draws POS-coloured
  word highlights, the furigana definition popup, and the latency HUD. The
  pure, Win32-free hit-testing and popup-content logic lives in
  `hover/mod.rs`, `hover/furigana.rs`, and `hover/sense.rs` so it can be
  unit-tested off-Windows.

### Temporal smoothing (reuse policy)

`SmoothingState` reuses the previous full detection while the *text regions*
are unchanged, skipping detect + recognize + analyze. It samples luma at the
previously detected text rects (an `Anchor`) rather than hashing the whole
frame, so an animated game background outside the text does not force a
re-OCR; the reuse is bounded by a max age (3 s) so any missed change
self-corrects. The anchor doubles as a capture-side `FrameProbe`: when reuse
is eligible, the worker hands it to the frame source, and the WGC path
samples the same points on the raw staging buffer - an unchanged scene skips
frame materialization entirely, so a stable-scene pass costs a map plus a few
hundred point samples instead of the full convert/retain/fingerprint path.
This is the only frame-reuse mechanism - `watch` does not run through a
generic pipeline scheduler.

## Real OCR runtime

The default runtime config is `nvidia_tensor_rt_then_cuda`, and the GPU path
is wired end to end with a graceful fallback chain **TensorRT -> CUDA -> CPU**
(`commit_session` in `ort_engine/mod.rs`):

1. **TensorRT EP** (FP16) builds one engine per model from a single
   dynamic-shape optimization profile, so the variable-width recognizer and
   variable-size detector are each served by *one* cached engine - no
   per-shape rebuild. Engines and the kernel timing cache persist under
   `cache/engines`; the multi-minute first build is paid once. CUDA is
   co-registered so any op TensorRT declines runs on the GPU rather than the
   CPU. The optimization profile constants are part of measured behavior:
   rebuilding with a different profile changes kernel selection and shifts
   detection output measurably with no code change (see `docs/accuracy.md`),
   so treat them as frozen.
2. **CUDA EP** is the fallback, tuned for the dynamic-shape pipeline
   (`tuned_cuda_ep`): `Heuristic` cuDNN conv-algorithm search (the default
   `Exhaustive` re-benchmarks every unseen input width - tens of ms per call)
   and TF32 tensor-core math.
3. **CPU EP** is the last resort for correctness without a GPU runtime.

TensorRT and CUDA need NVIDIA runtime DLLs next to the binary; ORT ships only
the provider shims. Run `scripts\bootstrap-dev.ps1 -GpuRuntimeOnly` once to
fetch pinned TensorRT 10 / CUDA 12 / cuDNN 9 wheels into `.gpu-runtime\bin`.
`build.rs` stages that cache into the active Cargo profile output dir on every
build, so debug and release binaries get the same provider DLL dependencies.

## Pipeline latency

The default path detects at native resolution (up to a 3840 px long side) for
recall on small game-menu labels, and runs the second local-contrast detection
pass. Measured on the reference NVIDIA GPU against true 4K game frames
(`docs/performance.md` holds the dated reference numbers):

| configuration | full OCR pass |
|---|---|
| CPU resize + FP32 CUDA (historical baseline) | ~464 ms |
| Low-res 1920 detection, TensorRT FP16 (optional config) | ~100 ms p95 |
| Native-4K dual-pass detection, TensorRT FP16 (default) | ~200 ms p95 typical frame; ~270 ms p99 on the densest 95-line menu |

Three design decisions carry the budget:

- **TensorRT FP16 fuses the mobile models' depthwise convolutions**, which on
  the CUDA EP were launch-bound (~33 ms of fixed kernel-launch overhead per
  recognizer call regardless of data). Per-batch recognizer inference is
  ~4-9 ms and accuracy-neutral versus FP32.
- **Detector preprocessing pads instead of resampling.** A 3840x2160 frame
  becomes a 3840x2176 tensor without touching the content pixels, and
  postprocessing maps boxes through the unpadded content extent so padding
  cannot shift or create user-visible boxes. Tiny or unusually shaped inputs
  still resize to the stride-aligned shape when padding would otherwise be a
  material fraction of the tensor. When a downscaled detector path *is*
  configured, the shrink uses a rayon-parallel coverage-weighted area
  resampler (`area_downscale`, ~8 ms instead of ~208 ms single-threaded
  bicubic), and area averaging preserves thin strokes that bicubic ringing
  destroys.
- **CPU postprocessing is kept off the critical path**: scanline union-find
  components instead of flood fill, and the local-contrast image build
  overlapped with the primary detector inference.

The recognizer always crops from the full-resolution frame, so recognition
quality is independent of any detector downscale. The old low-resolution fast
path remains available (`pipeline.detector_downscale = 0.5`,
`detector_min_long_side = 1920`, `detector_max_long_side = 1920`) when GPU
time matters more than small-label recall.

### Mobile vs. server models

The PP-OCRv5 **mobile** models are the default. The **server** variants are
several times heavier, overflow in FP16 (garbage output, so they require
FP32), and - measured on the labeled eval corpus - score *below* mobile
recognition (98.44% vs 98.73% characters). The corpus labels co-evolved with
the mobile reader, so a different reader shifts ambiguous lines in both
directions; `docs/accuracy.md` covers the finding. The supported way to use
server recognition is the optional per-line fallback recognizer described in
`docs/models.md`, which is off by default.

## Latency acceptance

Use `cargo run -- watch --metrics-interval-secs N` (or `--hud`) against a live
window as the acceptance loop; it reports rolling p50/p95/p99 for each stage
(capture, detect, recognize, analyze, present). On the default native-4K path
a typical full 4K pass is ~200 ms p95, with ~270 ms p99 on the densest menu
frames; the optional low-res path runs ~100 ms p95. A fast average is not
enough - large p95/p99 spikes mean the overlay will feel bad even if the mean
looks fine. `docs/performance.md` defines the before/after evidence expected
for latency changes.
