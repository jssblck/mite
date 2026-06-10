# OCR Model Setup

Mite reads text with two small neural networks that run on your graphics
card: a *detector* that finds where text is on screen, and a *recognizer*
that reads the characters inside each found line. This page covers which
model files mite uses, where they come from, and the optional heavier
variants. The default set works well for nearly everyone; the options exist
for people with GPU headroom to spare.

The default local model set is PP-OCRv5 mobile detection plus PP-OCRv5 mobile
recognition. The files are downloaded into `models\`, which is intentionally
ignored by git.

## Download

```powershell
.\scripts\bootstrap-dev.ps1 -ModelsOnly
.\scripts\bootstrap-dev.ps1 -ModelsOnly -IncludeServerModels
```

This reads `model-manifest.json`, downloads the ONNX files from RapidAI's
RapidOCR ModelScope repository, verifies the published SHA256 hashes, and
writes `models\MODELS.lock.json` with the exact local hashes.
`-IncludeServerModels` also fetches the optional heavier server detector and
recognizer.

## Mobile vs. server variants

The **mobile** models are the default and the recommended pairing with the
TensorRT FP16 path: fast enough to share a GPU with a running game, and
accuracy-neutral under FP16. The **server** variants
(`ch_PP-OCRv5_det_server.onnx`, `ch_PP-OCRv5_rec_server.onnx`, same
`v3.8.0/onnx/PP-OCRv5` path, same 18385-char dict) are several times heavier
and **produce garbage under FP16** (the larger layers overflow half
precision), so they must run in FP32.

Measured on the labeled eval corpus, server recognition scores *below* mobile
(98.44% vs 98.73% characters) at several times the per-line cost, and
whole-corpus server detection also fails to beat mobile. The corpus labels
co-evolved with the mobile reader, so a different reader moves ambiguous
lines in both directions; see `docs/accuracy.md` for the full finding. Server
models may still read better on text outside the labeled corpus, but there is
no measured evidence that swapping them in wholesale helps.

To run them anyway, point `models.detector_path` / `models.recognizer_path`
at the server files and set `runtime.fp16 = false`.

## Optional fallback recognizer

`models.fallback_recognizer_path` (off by default) loads a second, heavier
recognizer - intended for `ch_PP-OCRv5_rec_server.onnx` - that gives a second
opinion on lines the primary reads below 0.75 confidence. It always runs FP32
and its read wins only above 0.92 absolute confidence, so it touches a
handful of lines per frame. Measured on the labeled eval corpus this was
score-neutral (96.17% vs 96.18% aggregate), for the same co-evolution reason
as above. It may still help on text outside the labeled corpus if you have
the GPU headroom.

## Verify ONNX loading

```powershell
uv venv --python 3.11 .venv-models
uv pip install --python .\.venv-models\Scripts\python.exe onnxruntime
.\.venv-models\Scripts\python.exe .\scripts\verify-onnx-models.py
```

The verifier loads detector and recognizer with ONNX Runtime CPU execution
and prints model input/output signatures. It does not run OCR postprocessing.

## Provenance

The repo uses RapidAI's converted ONNX assets because they are already
published with checksums in RapidOCR's maintained `default_models.yaml`. The
official PaddlePaddle Hugging Face repos publish Paddle static inference
artifacts under Apache-2.0, and PaddleOCR's current docs describe ONNX
conversion through PaddleX's `paddle2onnx` plugin.

Primary source references:

- `PaddlePaddle/PP-OCRv5_mobile_det`
- `PaddlePaddle/PP-OCRv5_mobile_rec`
- `RapidAI/RapidOCR`
- `model-manifest.json`
