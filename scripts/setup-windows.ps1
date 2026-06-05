Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

New-Item -ItemType Directory -Force -Path "models" | Out-Null
New-Item -ItemType Directory -Force -Path "cache\engines" | Out-Null

Write-Host "Checking Rust..."
rustc --version
cargo --version

Write-Host "Checking NVIDIA..."
if (Get-Command nvidia-smi -ErrorAction SilentlyContinue) {
    nvidia-smi --query-gpu=name,driver_version,memory.total --format=csv,noheader,nounits
} else {
    Write-Warning "nvidia-smi was not found on PATH. Install/update NVIDIA drivers before using the NVIDIA runtime backend."
}

if (-not (Test-Path "mite.toml")) {
    cargo run -- init-config
}

Write-Host ""
Write-Host "Setup complete. Put ONNX models in:"
Write-Host "  $root\models\pp-ocrv5-mobile-det.onnx"
Write-Host "  $root\models\pp-ocrv5-mobile-rec.onnx"
Write-Host "  $root\models\pp-ocrv5-dict.txt"
Write-Host ""
Write-Host "For GPU acceleration via TensorRT (recommended; ~6x lower OCR latency),"
Write-Host "fetch the pinned GPU runtime once, then build:"
Write-Host "  .\scripts\install-gpu-runtime.ps1"
Write-Host "  cargo build --release"
Write-Host ""
Write-Host "Then run:"
Write-Host "  cargo run -- doctor"
