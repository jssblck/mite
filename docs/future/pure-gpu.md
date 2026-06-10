# Future direction: a pure-GPU-side vision pipeline

Status: **exploratory / not scheduled.** This captures the design space for moving
the whole capture → resize → detect → crop → recognize path onto the GPU with no
pixel data crossing to the CPU. It is written down so the trade-offs are not
re-derived from scratch next time. Nothing here is a commitment; the honest
conclusion is at the bottom.

## Why this document exists

After the TensorRT work (see `docs/architecture.md`), the remaining CPU work
in the pipeline is small and mostly parallelized, so the question came up: is
it worth making the pipeline *fully* GPU-resident, and is it even possible?
The arithmetic below was done on the low-res 1920 detector configuration
(~100 ms p95 live, ~55 ms controlled detect+recognize). The native-4K default
that shipped later spends *more* time in model inference (~200 ms p95
typical), which strengthens this document's conclusion: inference, not CPU
glue, is the wall.

Short answer: **possible — yes, it's a proven pattern; faster — yes, but only
~15–25% on mean latency today, bounded by model inference.** It becomes clearly
worth it only when (a) the models get dramatically faster (so CPU glue becomes the
dominant fraction), or (b) we specifically need lower tail-latency / higher OCR
fps / less CPU contention with the game. It is a full rewrite of the vision path.

## Where the data crosses today

Current flow (`wgc_capture.rs` -> `ort_engine/mod.rs`):

1. WGC delivers a `Direct3D11CaptureFrame` (BGRA texture) on its own `ID3D11Device`.
2. We `CopySubresourceRegion` it into a CPU-readable **staging texture**.
3. `map_staging_to_rgb` maps it and converts **BGRA→RGB** into a host `RgbImage`
   (`bgra_to_rgb`, rayon-parallel).
4. Detect: `area_downscale` (CPU, parallel) → `nchw_tensor` (CPU, parallel) →
   `Tensor::from_array` (host) → `session.run` (ORT uploads host→device, runs,
   copies output device→host).
5. Detector postprocessing: `detect_components_from_probability_map` (CPU
   scanline union-find) produces box rects. The optional local-contrast
   second detection pass builds its input image on a CPU worker thread
   overlapped with the primary detector inference.
6. Recognize: `crop_text_line` cuts boxes from the full-res host frame → resize →
   host tensor → `session.run`.
7. CTC decode (CPU, parallel) → text → lookup/overlay (CPU, Win32).

The pixel data makes exactly **one** clean GPU round trip (the host tensor ORT
copies in/out). The frame is read back to the CPU in full (~25 MB at 4K) because
the recognizer crops from the full-resolution image.

## What "never touches the CPU" really means

It can't be literally never: the **results** (box rects, recognized text, token
indices) must come back — but that's kilobytes, not the 25 MB frame. The
achievable target is *"pixel data never leaves the GPU; only tiny metadata
returns."* This is exactly the **NVIDIA DeepStream / CV-CUDA / Holoscan** pattern
used by video-analytics and self-driving stacks: keep frames in GPU memory,
preprocess with GPU kernels, run detection+recognition as TensorRT engines with
device I/O binding, and bring back only structured outputs. The architecture is
proven; the question is build cost vs. payoff.

## Stage-by-stage, fully on GPU

| stage | today (CPU) | GPU-resident version |
|---|---|---|
| capture | readback 4K BGRA→RGB | keep the WGC D3D11 texture on-device; no readback |
| preprocess | `area_downscale` + `nchw_tensor` (~10 ms) | one fused kernel: BGRA→RGB + resize + NCHW-normalize (sub-ms) |
| detect | host tensor → ORT (host→device upload) | TensorRT with input **I/O-bound** to the device buffer |
| det. postproc | `detect_components_from_probability_map` (CPU union-find, a few ms) | GPU connected-components, **or** read back only the small prob map |
| crop | `crop_text_line` per box on full-res host image | GPU gather + batched-resize kernel producing the padded NCHW batch on-device |
| recognize | host tensor → ORT | TensorRT, device I/O bound |
| CTC decode | CPU argmax over timesteps×classes | GPU argmax; transfer back only the index sequences |
| lookup/overlay | CPU (Lindera/JMdict, Win32) | **stays CPU** — operates on KB of text |

The CPU irreducibly keeps control flow, the dictionary/segmentation/lookup, and
the overlay (layered Win32 window + hover hit-testing). Those are tiny and belong
on the CPU. "All-GPU" means the *vision* path, with a thin metadata bridge to the
existing CPU lookup/overlay.

## The hard seam: two GPU runtimes that don't share memory

The difficulty is not any one kernel — it's that capture lives in **D3D11** and
inference lives in **CUDA/TensorRT**, and a D3D11 texture is not a CUDA pointer.
Bridging them needs explicit interop (`cudaGraphicsD3D11RegisterResource`, or a
shared NT handle + `cudaImportExternalMemory` + keyed mutex), and the CUDA work
must run on the *same context* that imported the memory — but the `ort` crate owns
and hides its CUDA context/stream/allocator. See `docs/architecture.md`'s
device-resident discussion for the detailed failure modes (cross-context access,
`unsafe` device-pointer I/O binding, cross-thread/stream fencing, untestable
hardware-specific bugs).

A full rewrite gets to **pick one GPU API end-to-end and erase the seam:**

- **All-CUDA / TensorRT.** Capture either via a CUDA-side path or the D3D11→CUDA
  interop bridge; preprocess with CUDA/NPP/CV-CUDA kernels; infer on TensorRT with
  device I/O binding. **Fastest inference; hardest seam** (still need the bridge at
  capture, plus a new CUDA-kernel build dependency that re-couples us to a CUDA
  version — see the CUDA 12-vs-13 notes).
- **All-D3D / DirectML.** Capture is already a D3D11 texture; preprocess with an
  HLSL compute shader; infer on ONNX Runtime's **DirectML** EP. *Everything stays
  in D3D — no CUDA interop at all.* **Easiest to build correctly on Windows; the
  tradeoff is DirectML is generally slower than TensorRT** for these models.

That choice is the crux of any rewrite: DirectML buys a clean single-API zero-copy
pipeline at some inference cost; CUDA/TRT keeps the fast inference but pays the
interop complexity once.

## Is it faster? The arithmetic

(Measured on the low-res 1920 configuration; the native-4K default has an even
larger inference share, so the conclusion only gets stronger.) Controlled
detect+recognize ≈ 55 ms = inference ~41 ms (det 8 + rec 33) + CPU glue
~14 ms (resize 8 + tensor 2 + components 2 + crop 2). Full residency removes the
~14 ms of glue plus the readback/upload copies — call it ~15–20 ms.

- Mean drops ~**20–25%** (controlled ~55→~42 ms; live ~100→~80 ms once the
  unavoidable compositor frame-wait, present, and analyze are added back).
- **The recognizer inference (~33 ms) is the wall, and residency does not touch
  it.** Hence: a latency-only justification needs the models to get dramatically
  faster first (INT8 / distillation / a faster recognizer), at which point the CPU
  glue becomes the dominant fraction and residency's percentage grows.

### Why mean latency undersells it

1. **Tail latency / jitter.** Removing host copies and CPU scheduling kills p95/p99
   spikes — the overlay-feel metric the architecture doc explicitly cares about.
2. **CPU headroom for the game.** Moves ~15–25 ms of per-pass CPU off the cores the
   game wants — the "don't hurt the game" Pareto axis.
3. **Throughput ceiling.** Sustains a much lower `--refresh-ms` (higher OCR fps)
   without saturating the CPU.
4. **Enables a CUDA graph.** A *fixed-shape* fully-GPU pipeline can be captured as a
   CUDA graph, collapsing per-kernel launch overhead into one replay. We can't use
   CUDA graphs today because of dynamic shapes + CPU glue in the middle.

## Incremental path (lower-risk stepping stones)

If revisited, do not start with full interop. In rough order of value/risk:

1. **INT8 recognizer (no residency).** TensorRT EP already supports it
   (`with_int8` + a calibration table). INT8 sidesteps the fp16 overflow that
   blocks the server models *and* speeds inference — the highest-leverage move for
   the accuracy axis, and it shrinks the inference wall that bounds everything
   else. Calibrate on a broad set of game frames; re-validate with labeled
   captures in the private `eval\` submodule through `cargo run -- eval`.
2. **D3D shader downscale (no CUDA).** Render the captured texture to a 1920 target
   on the WGC device; read *that* back for the detector (4× less readback + kills
   the CPU resize), still reading full 4K for recognizer crops. No CUDA interop;
   modest win.
3. **Skip RGB materialization.** Index the BGRA buffer directly in crop/resize/
   tensor instead of building an `RgbImage`. Pure-CPU, removes a 25 MB alloc + a
   conversion pass, but touches many call sites (overlay, debug-capture,
   contrast-stretch all assume `RgbImage`).
4. **Single-API zero-copy rewrite.** Only after 1–3 and a faster recognizer make
   the CPU glue the dominant cost. Pick DirectML (clean, slower infer) or
   CUDA/TRT-with-interop (fast infer, hard seam) per the trade above.

## Bottom line

In theory: yes, possible (proven DeepStream/CV-CUDA pattern) and yes, somewhat
faster — but the payoff today is ~20% mean latency plus real wins in jitter and CPU
headroom, behind a full rewrite. It crosses the worth-it threshold when the models
get dramatically faster (making CPU glue dominant) or when tail-latency / fps /
game-CPU-contention become the explicit goal. Until then, the higher-leverage work
is faster/lighter models (INT8, distillation) — not pipeline residency.
