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

## Reference Numbers (2026-06-09)

Offline full-pass profile on labeled 4K eval captures (reference NVIDIA GPU,
PP-OCRv5 mobile, TensorRT FP16, `cargo run --release --example profile_ocr`):

```text
dense 95-line menu frame (capture-1780033429968):
  before: detect p50 207 / p95 252 / p99 260   e2e p50 302 / p95 356 / p99 367
  after:  detect p50 114 / p95 140 / p99 160   e2e p50 205 / p95 244 / p99 267
typical 40-line frame (capture-1780033168244):
  after:  detect p50 113 / p95 149              e2e p50 171 / p95 202 / p99 246
```

The "after" build replaces the per-pixel flood fill in detector
postprocessing with scanline union-find connected components (identical
output, several times faster) and overlaps the CPU local-contrast image build
with the primary detector inference. A half-resolution local-mean variant was
measured and rejected: it saved ~5 ms wall but cost 0.10 aggregate eval
points. Likewise PP-OCRv5 server recognition was measured against the eval
corpus and scored below mobile recognition (98.44% vs 98.73% characters) at
several times the per-line cost, so mobile remains the recognizer.
