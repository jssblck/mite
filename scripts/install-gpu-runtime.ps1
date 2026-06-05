param(
    [switch]$SkipTargetStage
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

# Fetches the redistributable TensorRT/CUDA/cuDNN DLLs used by ONNX Runtime's
# TensorRT and CUDA execution providers, stores them in .gpu-runtime\bin, and
# stages them next to already-built mite binaries. Future Cargo builds stage the
# same cache automatically through build.rs.

$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$venv = Join-Path $root ".venv-models"
$python = Join-Path $venv "Scripts\python.exe"
$runtimeRoot = Join-Path $root ".gpu-runtime"
$runtimeBin = Join-Path $runtimeRoot "bin"
$manifestPath = Join-Path $runtimeRoot "manifest.json"

$packages = @(
    "tensorrt-cu12==10.16.1.11",
    "nvidia-cuda-runtime-cu12==12.9.79",
    "nvidia-cuda-nvrtc-cu12==12.9.86",
    "nvidia-cublas-cu12==12.9.2.10",
    "nvidia-cudnn-cu12==9.22.0.52"
)

function Copy-DllsFrom {
    param(
        [Parameter(Mandatory = $true)]
        [string]$SourceDir,
        [Parameter(Mandatory = $true)]
        [string]$Label
    )

    if (-not (Test-Path $SourceDir)) {
        throw "$Label DLL directory not found at $SourceDir"
    }

    $files = @(Get-ChildItem -Path $SourceDir -Filter "*.dll" -File -ErrorAction Stop)
    if (-not $files) {
        throw "$Label DLL directory contains no DLLs: $SourceDir"
    }

    foreach ($file in $files) {
        Copy-Item -LiteralPath $file.FullName -Destination $runtimeBin -Force
    }

    Write-Host "Cached $($files.Count) $Label DLL(s) from $SourceDir"
}

function Copy-DllsFromOptional {
    param(
        [Parameter(Mandatory = $true)]
        [string]$SourceDir,
        [Parameter(Mandatory = $true)]
        [string]$Label
    )

    if (Test-Path $SourceDir) {
        Copy-DllsFrom -SourceDir $SourceDir -Label $Label
    }
}

function Assert-DllsPresent {
    param(
        [Parameter(Mandatory = $true)]
        [string[]]$Names
    )

    $missing = @(
        foreach ($name in $Names) {
            if (-not (Test-Path (Join-Path $runtimeBin $name))) {
                $name
            }
        }
    )

    if ($missing) {
        throw "GPU runtime cache is missing required DLLs: $($missing -join ', ')"
    }
}

New-Item -ItemType Directory -Path $runtimeBin -Force | Out-Null

if (-not (Test-Path $python)) {
    Write-Host "Creating model/runtime venv at $venv ..."
    uv venv --python 3.11 $venv
}

Write-Host "Installing GPU runtime wheels..."
uv pip install --python $python $packages

$sitePackages = Join-Path $venv "Lib\site-packages"
$tensorrtLibs = Join-Path $sitePackages "tensorrt_libs"
$nvidiaRoot = Join-Path $sitePackages "nvidia"

Copy-DllsFrom -SourceDir $tensorrtLibs -Label "TensorRT"
Copy-DllsFrom -SourceDir (Join-Path $nvidiaRoot "cuda_runtime\bin") -Label "CUDA runtime"
Copy-DllsFrom -SourceDir (Join-Path $nvidiaRoot "cuda_nvrtc\bin") -Label "CUDA NVRTC"
Copy-DllsFrom -SourceDir (Join-Path $nvidiaRoot "cublas\bin") -Label "cuBLAS"
Copy-DllsFrom -SourceDir (Join-Path $nvidiaRoot "cudnn\bin") -Label "cuDNN"

# Some future NVIDIA wheels may split additional runtime DLLs into sibling
# packages. Cache any DLL-bearing directories under nvidia\ without requiring a
# code change here.
$knownNvidiaDirs = @("cuda_runtime", "cuda_nvrtc", "cublas", "cudnn")
foreach ($dir in Get-ChildItem -Path $nvidiaRoot -Directory -ErrorAction SilentlyContinue) {
    if ($knownNvidiaDirs -contains $dir.Name) {
        continue
    }
    Copy-DllsFromOptional -SourceDir (Join-Path $dir.FullName "bin") -Label $dir.Name
}

Assert-DllsPresent -Names @(
    "nvinfer_10.dll",
    "nvonnxparser_10.dll",
    "nvinfer_plugin_10.dll",
    "cudart64_12.dll",
    "cublas64_12.dll",
    "cublasLt64_12.dll",
    "cudnn64_9.dll",
    "cudnn_ops64_9.dll",
    "cudnn_cnn64_9.dll"
)

$cachedDlls = Get-ChildItem -Path $runtimeBin -Filter "*.dll" -File |
    Sort-Object Name |
    Select-Object -ExpandProperty Name

if (-not $SkipTargetStage) {
    $targetDirs = @(
        (Join-Path $root "target\debug"),
        (Join-Path $root "target\debug\deps"),
        (Join-Path $root "target\debug\examples"),
        (Join-Path $root "target\release"),
        (Join-Path $root "target\release\deps"),
        (Join-Path $root "target\release\examples")
    ) | Where-Object { Test-Path $_ }

    foreach ($dst in $targetDirs) {
        foreach ($dll in Get-ChildItem -Path $runtimeBin -Filter "*.dll" -File) {
            Copy-Item -LiteralPath $dll.FullName -Destination $dst -Force
        }
        Write-Host "Staged GPU runtime DLLs into $dst"
    }

    if (-not $targetDirs) {
        Write-Warning "No Cargo target output dirs found yet. build.rs will stage the cache on the next cargo build."
    }
}

$manifest = [ordered]@{
    created_by = "scripts/install-gpu-runtime.ps1"
    runtime_bin = $runtimeBin
    packages = $packages
    dll_count = $cachedDlls.Count
    dlls = $cachedDlls
}
$manifest | ConvertTo-Json -Depth 4 | Set-Content -Path $manifestPath -Encoding UTF8

Write-Host ""
Write-Host "GPU runtime ready in $runtimeBin"
Write-Host "Manifest: $manifestPath"
Write-Host "Verify with:"
Write-Host "  cargo run -- doctor"
Write-Host "  .\scripts\precommit.ps1 -IncludeEval"
