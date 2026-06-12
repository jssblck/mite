"""Quantize the PP-OCRv5 mobile detector/recognizer to explicit-QDQ INT8.

Produces models/pp-ocrv5-mobile-det-int8.onnx and
models/pp-ocrv5-mobile-rec-int8.onnx via onnxruntime static quantization,
calibrated on real captures from the private eval/ submodule so activation
ranges match deployment inputs (game frames, not natural photos).

TensorRT 10+ deprecates implicit INT8 calibration tables; explicit Q/DQ models
are the supported path, and the TensorRT EP builds INT8 engines from them
directly (mite enables this with runtime.int8 = true).

Preprocessing mirrors src/ort_engine/mod.rs exactly:
  detector:   x/255, mean [0.485,0.456,0.406], std [0.229,0.224,0.225],
              NCHW, zero-padded to a multiple of 32 (content pixels untouched)
  recognizer: (x/255 - 0.5)/0.5, height 48, width = round(48*aspect)
              clamped to [16, 960] and rounded up to a multiple of 8,
              bilinear resize

Usage (from the repo root):
  .venv-models\\Scripts\\python.exe scripts\\quantize-models.py [--det-only|--rec-only]
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

import numpy as np
from PIL import Image
from onnxruntime.quantization import (
    CalibrationDataReader,
    CalibrationMethod,
    QuantFormat,
    QuantType,
    quantize_static,
)
from onnxruntime.quantization.shape_inference import quant_pre_process

ROOT = Path(__file__).resolve().parents[1]
EVAL_ROOT = ROOT / "eval"
MODELS = ROOT / "models"

DET_MEAN = np.array([0.485, 0.456, 0.406], dtype=np.float32)
DET_STD = np.array([0.229, 0.224, 0.225], dtype=np.float32)
DET_SIZE_MULTIPLE = 32
DET_MAX_LONG_SIDE = 3840

REC_HEIGHT = 48
REC_MIN_WIDTH = 16
REC_MAX_WIDTH = 960
REC_WIDTH_MULTIPLE = 8

# Calibration sample counts. Detector samples are full 4K tensors run on the
# CPU EP, so the count is the main knob on calibration wall time.
DET_SAMPLES = 16
REC_SAMPLE_CAPTURES = 48
REC_CROPS_PER_CAPTURE = 8


def capture_dirs() -> list[Path]:
    dirs = sorted(
        d
        for collection in EVAL_ROOT.iterdir()
        if collection.is_dir() and not collection.name.startswith(".")
        for d in collection.iterdir()
        if d.is_dir() and (d / "underlying.png").exists() and (d / "eval.json").exists()
    )
    if not dirs:
        sys.exit("no eval captures found; initialize the eval submodule first")
    return dirs


def stride_sample(items: list, count: int) -> list:
    if len(items) <= count:
        return items
    step = len(items) / count
    return [items[int(i * step)] for i in range(count)]


def det_preprocess(image: Image.Image) -> np.ndarray:
    width, height = image.size
    long_side = max(width, height)
    if long_side > DET_MAX_LONG_SIDE:
        scale = DET_MAX_LONG_SIDE / long_side
        width = max(1, round(width * scale))
        height = max(1, round(height * scale))
        image = image.resize((width, height), Image.BICUBIC)
    padded_w = -(-width // DET_SIZE_MULTIPLE) * DET_SIZE_MULTIPLE
    padded_h = -(-height // DET_SIZE_MULTIPLE) * DET_SIZE_MULTIPLE
    pixels = np.asarray(image.convert("RGB"), dtype=np.float32) / 255.0
    normalized = (pixels - DET_MEAN) / DET_STD
    tensor = np.zeros((1, 3, padded_h, padded_w), dtype=np.float32)
    tensor[0, :, :height, :width] = normalized.transpose(2, 0, 1)
    return tensor


def rec_preprocess(crop: Image.Image) -> np.ndarray:
    width, height = crop.size
    ratio = width / max(height, 1)
    target_w = int(np.clip(round(REC_HEIGHT * ratio), REC_MIN_WIDTH, REC_MAX_WIDTH))
    target_w = -(-target_w // REC_WIDTH_MULTIPLE) * REC_WIDTH_MULTIPLE
    resized = crop.convert("RGB").resize((target_w, REC_HEIGHT), Image.BILINEAR)
    pixels = np.asarray(resized, dtype=np.float32) / 255.0
    normalized = (pixels - 0.5) / 0.5
    return normalized.transpose(2, 0, 1)[np.newaxis, ...]


class DetReader(CalibrationDataReader):
    def __init__(self, input_name: str):
        captures = stride_sample(capture_dirs(), DET_SAMPLES)
        self.input_name = input_name
        self.paths = [c / "underlying.png" for c in captures]
        self.index = 0

    def get_next(self):
        if self.index >= len(self.paths):
            return None
        path = self.paths[self.index]
        self.index += 1
        print(f"  det calibration {self.index}/{len(self.paths)}: {path.parent.name}")
        return {self.input_name: det_preprocess(Image.open(path))}


class RecReader(CalibrationDataReader):
    def __init__(self, input_name: str):
        self.input_name = input_name
        self.samples = []
        for capture in stride_sample(capture_dirs(), REC_SAMPLE_CAPTURES):
            labels = json.loads((capture / "eval.json").read_text(encoding="utf-8"))
            image = Image.open(capture / "underlying.png").convert("RGB")
            detections = stride_sample(
                labels.get("detections", []), REC_CROPS_PER_CAPTURE
            )
            for det in detections:
                b = det["bounds"]
                left = max(0, int(np.floor(b["x"])))
                top = max(0, int(np.floor(b["y"])))
                right = min(image.width, int(np.ceil(b["x"] + b["width"])))
                bottom = min(image.height, int(np.ceil(b["y"] + b["height"])))
                if right - left < 2 or bottom - top < 2:
                    continue
                self.samples.append(image.crop((left, top, right, bottom)))
        print(f"  rec calibration: {len(self.samples)} line crops")
        self.index = 0

    def get_next(self):
        if self.index >= len(self.samples):
            return None
        sample = self.samples[self.index]
        self.index += 1
        return {self.input_name: rec_preprocess(sample)}


def input_name(model_path: Path) -> str:
    import onnxruntime as ort

    session = ort.InferenceSession(
        str(model_path), providers=["CPUExecutionProvider"]
    )
    return session.get_inputs()[0].name


def quantize(model: Path, reader_cls, output: Path) -> None:
    pre = output.with_suffix(".pre.onnx")
    print(f"preprocessing {model.name}...")
    # Symbolic shape inference chokes on the Paddle-exported dynamic shapes;
    # the ONNX-level optimization pass is what quantization actually needs.
    quant_pre_process(str(model), str(pre), skip_symbolic_shape=True)
    print(f"calibrating + quantizing {model.name} -> {output.name}")
    quantize_static(
        model_input=str(pre),
        model_output=str(output),
        calibration_data_reader=reader_cls(input_name(model)),
        quant_format=QuantFormat.QDQ,
        activation_type=QuantType.QInt8,
        weight_type=QuantType.QInt8,
        # Conv only: convolutions are the bulk of both models' compute, conv
        # weights quantize per-channel on axis 0 (which TensorRT's parser
        # accepts), and leaving MatMul/Gather/Mul constants in float avoids
        # the per-channel DequantizeLinear axes TensorRT rejects
        # (importerUtils convertAxis failures on helper.constant.* nodes).
        # Everything left unquantized runs FP16 in the mixed engine.
        op_types_to_quantize=["Conv"],
        per_channel=True,
        calibrate_method=CalibrationMethod.MinMax,
        # TensorRT only accepts symmetric INT8 Q/DQ (zero point must be 0).
        extra_options={
            "ActivationSymmetric": True,
            "WeightSymmetric": True,
        },
    )
    pre.unlink(missing_ok=True)
    print(f"wrote {output} ({output.stat().st_size / 1e6:.1f} MB)")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--det-only", action="store_true")
    parser.add_argument("--rec-only", action="store_true")
    args = parser.parse_args()

    if not args.rec_only:
        quantize(
            MODELS / "pp-ocrv5-mobile-det.onnx",
            DetReader,
            MODELS / "pp-ocrv5-mobile-det-int8.onnx",
        )
    if not args.det_only:
        quantize(
            MODELS / "pp-ocrv5-mobile-rec.onnx",
            RecReader,
            MODELS / "pp-ocrv5-mobile-rec-int8.onnx",
        )


if __name__ == "__main__":
    main()
