# Performance Evidence

Mite aims to put definitions on screen fast enough that the wait is never
noticeable, while leaving most of the graphics card free for the game. For
users, the practical summary is in the Reference Numbers section at the
bottom: a typical full read of a 4K frame takes about a fifth of a second.
The rest of this page is the working rule for people changing the code.

Latency changes should be tied to measured p50/p95/p99 results from the same
target window and model/runtime configuration before and after the change.
The goal is not a ceremony-heavy benchmark suite; it is to keep performance
claims honest and make regressions visible.

## Live Latency Loop

Use the watch metrics stream against a real window:

```powershell
cargo run --release -- watch --title "Window title" --auto --metrics-interval-secs 5
```

For worst-case full-pass measurements, disable smoothing:

```powershell
cargo run --release -- watch --title "Window title" --auto --no-smoothing --metrics-interval-secs 5
```

Capture enough samples for the rolling window to stabilize. Record:

- GPU and model variant: for example, reference NVIDIA GPU, PP-OCRv5 mobile, TensorRT FP16.
- Capture backend: `auto`, `wgc`, or `screenshot`.
- Resolution and target window type: for example, 3840x2160 DirectX game.
- Whether smoothing was enabled.
- The final `metrics[...]` line, especially total p50/p95/p99 and per-stage
  p95/p99.

## Acceptance Rule

For performance-motivated changes, include before/after metrics in the change
summary. A useful claim has this shape:

```text
before: metrics[30s n=50] total 68/103/121 detect 16/29/35 recognize 35/53/65 ...
after:  metrics[30s n=50] total 60/84/96  detect 11/20/23 recognize 34/51/62 ...
target: reference NVIDIA GPU, 3840x2160, WGC, mobile models, TensorRT FP16, --no-smoothing
```

If a change is about responsiveness rather than raw stage time, include p95/p99,
drop count, frame age, and whether the overlay remains responsive while OCR is in
flight.

## Accuracy Coupling

Latency wins are not complete if they quietly hurt recognition or lookup. For
OCR, detector, recognizer, text-correction, morphology, dictionary, or eval
changes, initialize the private `eval\` submodule if needed and run a labeled
real capture:

```powershell
git submodule update --init eval
git -C eval lfs pull
```

```powershell
cargo run -- eval `
  --image eval\collection-name\capture-<ts>\underlying.png `
  --labels eval\collection-name\capture-<ts>\eval.json `
  --out target\eval\capture-<ts>.json
```

Use the aggregate scorer when a change should be judged against every labeled
capture in the private `eval\` submodule:

```powershell
cargo run -- eval-corpus `
  --root eval `
  --out target\eval\corpus-summary.json `
  --allow-failures
```

The eval output is the accuracy side of the performance trade. The local
precommit script still supports `.\scripts\precommit.ps1 -IncludeEval` for a
strict perfect-score gate over each image.

## Reference Numbers (2026-06-10)

Offline full-pass profile on labeled 4K eval captures (reference NVIDIA GPU,
PP-OCRv5 mobile, TensorRT FP16, `cargo run --release --example profile_ocr`,
interleaved A/B on an idle GPU, 2 rounds of 30 iterations):

```text
dense 95-line menu frame (capture-1780033429968):
  before: detect p50 113 / p95 132   recognize p50 86 / p95 102   e2e p50 199
  after:  detect p50 101 / p95 114   recognize p50  74 / p95  93  e2e p50 173
typical 40-line frame (capture-1780033168244):
  before: detect p50 114 / p95 141   recognize p50 55 / p95 74    e2e p50 168
  after:  detect p50 100 / p95 123   recognize p50 48 / p95 62    e2e p50 148
```

The "after" build restructures stage scheduling so the GPU never waits on CPU
glue (eval corpus bit-identical, aggregate 96.18% before and after):

- The local-contrast detector image build is a parallel sliding-window box
  mean (exact-equal to the old integral image, 56 ms -> 20 ms at 4K), so it
  comfortably hides under the primary detector inference and returns ~36 ms
  of per-pass CPU time to the game.
- The second (low-contrast) detector inference launches the moment the
  primary inference returns: the enhanced image and its tensor are built on a
  worker thread during primary inference, and the primary probability map's
  postprocessing runs concurrently with the second inference.
- Recognizer batches flow through a pack -> infer -> decode thread pipeline
  with identical batch composition, so tensor packing and CTC decode no
  longer serialize with GPU inference.

Tail behavior at this build, characterized over 200 iterations on the dense
frame (idle GPU): detect p50 100 / p95 118 / p99 143, recognize p50 71 /
p95 87 / p99 120 - p99 stays within ~1.4x of p50 with no multi-hundred-ms
stalls. On the live `watch` path, stable scenes additionally skip frame
materialization entirely (the smoothing anchor is evaluated on the WGC
staging buffer before any conversion), so steady-state reuse passes report
detect/recognize/analyze at ~0 ms and the capture stage is dominated by the
deliberate frame-wait, not compute.

Measured and rejected on this hardware:

- **INT8 (QDQ) detector/recognizer** (2026-06-10): the Conv-only symmetric
  per-channel quantized models built TensorRT INT8 engines successfully but
  lost on both axes - detect ran ~38% slower with a noisy probability map
  (256 candidate boxes, the safety cap, vs 153), recognize ran ~28% slower
  and passed 78 lines vs FP16's 95. TensorRT FP16 already fuses the mobile
  models' depthwise convolutions near-optimally; Q/DQ reformat overhead
  swamps the INT8 math gain. `scripts/quantize-models.py` and the
  `runtime.int8_*` flags remain for cheap re-testing on future models.
- A half-resolution local-mean variant of the low-contrast detector image:
  saved ~5 ms wall but cost 0.10 aggregate eval points (2026-06-09).
- PP-OCRv5 server recognition: scored below mobile (98.44% vs 98.73%
  characters) at several times the per-line cost (2026-06-09).

Earlier 2026-06-09 work replaced the per-pixel flood fill in detector
postprocessing with scanline union-find connected components (identical
output, several times faster).
