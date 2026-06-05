# Performance Evidence

Mite latency changes should be tied to measured p50/p95/p99 results from the
same target window and model/runtime configuration before and after the change.
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

Use `.\scripts\precommit.ps1 -IncludeEval` to run every labeled
capture in the private `eval\` submodule. The eval output is the accuracy side of the
performance trade.
