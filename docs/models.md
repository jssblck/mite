# OCR Model Setup

The default local model set is PP-OCRv5 mobile detection plus PP-OCRv5 mobile recognition. The files are downloaded into `models\`, which is intentionally ignored by git.

## Mobile vs. server variants

The **mobile** models are the default and the recommended pairing with the
TensorRT FP16 path: a full 4K pass is ~100 ms p95 and recognition is
accuracy-neutral versus FP32. The **server** variants (`ch_PP-OCRv5_det_server.onnx`,
`ch_PP-OCRv5_rec_server.onnx`, same `v3.8.0/onnx/PP-OCRv5` path, same 18385-char
dict) read body text more accurately but are much heavier and **produce garbage
under FP16** (the larger layers overflow half precision), so they must run in
FP32 — slower and more GPU-hungry. To use them, point
`models.detector_path`/`models.recognizer_path` at the server files and set
`runtime.fp16 = false`. See the README's "TensorRT acceleration" section for the
download commands and `docs/architecture.md` for the measured trade-off.

## Download

```powershell
.\scripts\download-models.ps1
```

This reads `model-manifest.json`, downloads the ONNX files from RapidAI's RapidOCR ModelScope repository, verifies the published SHA256 hashes for the ONNX artifacts, and writes `models\MODELS.lock.json` with the exact local hashes.

## Verify ONNX Loading

```powershell
uv venv --python 3.11 .venv-models
uv pip install --python .\.venv-models\Scripts\python.exe onnxruntime
.\.venv-models\Scripts\python.exe .\scripts\verify-onnx-models.py
```

The verifier loads detector and recognizer with ONNX Runtime CPU execution and prints model input/output signatures. It does not run OCR postprocessing yet.

## Provenance

The repo uses RapidAI's converted ONNX assets because they are already published with checksums in RapidOCR's maintained `default_models.yaml`. The official PaddlePaddle Hugging Face repos publish Paddle static inference artifacts under Apache-2.0, and PaddleOCR's current docs describe ONNX conversion through PaddleX's `paddle2onnx` plugin.

Primary source references:

- `PaddlePaddle/PP-OCRv5_mobile_det`
- `PaddlePaddle/PP-OCRv5_mobile_rec`
- `RapidAI/RapidOCR`
- `model-manifest.json`
