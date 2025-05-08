#!/bin/sh
set -e

MODEL_DIR="models"
mkdir -p "$MODEL_DIR"

download() {
  url="$1"
  out="$2"
  if [ -f "$out" ]; then
    echo "[✓] $(basename "$out") already exists"
  else
    echo "[↓] Downloading $(basename "$out")..."
    curl -L "$url" -o "$out"
  fi
}

download "https://www.modelscope.cn/models/RapidAI/RapidOCR/resolve/master/onnx/PP-OCRv4/det/ch_PP-OCRv4_det_server_infer.onnx" "$MODEL_DIR/ch_PP-OCRv4_det_server_infer.onnx"
download "https://www.modelscope.cn/models/RapidAI/RapidOCR/resolve/master/onnx/PP-OCRv4/rec/japan_PP-OCRv4_rec_infer.onnx" "$MODEL_DIR/japan_PP-OCRv4_rec_infer.onnx"
download "https://www.modelscope.cn/models/RapidAI/RapidOCR/resolve/master/onnx/PP-OCRv4/rec/ch_PP-OCRv4_rec_server_infer.onnx" "$MODEL_DIR/ch_PP-OCRv4_rec_server_infer.onnx"
download "https://www.modelscope.cn/models/RapidAI/RapidOCR/resolve/master/onnx/PP-OCRv4/cls/ch_ppocr_mobile_v2.0_cls_infer.onnx" "$MODEL_DIR/ch_ppocr_mobile_v2.0_cls_infer.onnx"
download "https://raw.githubusercontent.com/PaddlePaddle/PaddleOCR/refs/heads/main/ppocr/utils/dict/japan_dict.txt" "$MODEL_DIR/japan_dict.txt"
download "https://raw.githubusercontent.com/PaddlePaddle/PaddleOCR/refs/heads/main/ppocr/utils/ppocr_keys_v1.txt" "$MODEL_DIR/ppocr_keys_v1.txt"

echo "[✔] All models and dictionary downloaded to: $MODEL_DIR"
