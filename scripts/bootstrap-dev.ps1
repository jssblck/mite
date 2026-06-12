param(
    [switch]$ModelsOnly,
    [switch]$GpuRuntimeOnly,
    [switch]$HooksOnly,
    [switch]$EvalDataOnly,
    [switch]$SkipModelDownload,
    [switch]$IncludeServerModels,
    [switch]$SkipGpuRuntime,
    [switch]$SkipBuild,
    [switch]$SkipDoctor,
    [switch]$SkipHooks,
    [switch]$IncludeEvalData,
    [switch]$Release,
    [switch]$SkipDriverCheck,
    [switch]$SkipTargetStage
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$focusedModes = @(
    @($ModelsOnly, $GpuRuntimeOnly, $HooksOnly, $EvalDataOnly) |
        Where-Object { $_ }
)
if ($focusedModes.Count -gt 1) {
    throw "Choose at most one focused mode: -ModelsOnly, -GpuRuntimeOnly, -HooksOnly, or -EvalDataOnly."
}

function Invoke-Step {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Name,
        [Parameter(Mandatory = $true)]
        [scriptblock]$Body
    )

    Write-Host ""
    Write-Host "==> $Name"
    & $Body
}

function Assert-Command {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Name,
        [Parameter(Mandatory = $true)]
        [string]$InstallHint
    )

    if (-not (Get-Command $Name -ErrorAction SilentlyContinue)) {
        throw "$Name was not found on PATH. $InstallHint"
    }
}

function Assert-NvidiaDriver {
    Assert-Command "nvidia-smi" "Install or update the NVIDIA display driver, then open a new terminal."
    $output = & nvidia-smi --query-gpu=name,driver_version,memory.total --format=csv,noheader,nounits 2>&1
    if ($LASTEXITCODE -ne 0) {
        throw "nvidia-smi failed. Install or repair the NVIDIA display driver before using the NVIDIA runtime backend.`n$output"
    }
    Write-Host "NVIDIA driver detected:"
    $output | ForEach-Object { Write-Host "  $_" }
}

function Resolve-RepoPath {
    param([Parameter(Mandatory = $true)][string]$Path)
    return Join-Path $root ($Path -replace "/", [System.IO.Path]::DirectorySeparatorChar)
}

function Get-Sha256 {
    param([Parameter(Mandatory = $true)][string]$Path)
    return (Get-FileHash -Algorithm SHA256 -LiteralPath $Path).Hash.ToLowerInvariant()
}

function Get-ManifestAsset {
    param([Parameter(Mandatory = $true)]$Asset)

    $target = Resolve-RepoPath $Asset.local_path
    $targetDir = Split-Path -Parent $target
    New-Item -ItemType Directory -Force -Path $targetDir | Out-Null

    $isArchive = $Asset.PSObject.Properties.Name -contains "archive" -and $null -ne $Asset.archive
    $sourceUrl = if ($isArchive) { $Asset.archive.url } else { $Asset.url }

    $needsDownload = $true
    if (Test-Path -LiteralPath $target) {
        if ($null -ne $Asset.sha256 -and $Asset.sha256 -ne "") {
            $currentHash = Get-Sha256 $target
            $needsDownload = $currentHash -ne $Asset.sha256.ToLowerInvariant()
            if ($needsDownload) {
                Write-Warning "Checksum mismatch for $($Asset.local_path); re-downloading."
            }
        } else {
            $needsDownload = $false
        }
    }

    if ($needsDownload) {
        Write-Host "Downloading $($Asset.id) -> $($Asset.local_path)"
        if ($isArchive) {
            $tempDir = Join-Path ([System.IO.Path]::GetTempPath()) ("mite-" + [System.Guid]::NewGuid().ToString("N"))
            New-Item -ItemType Directory -Force -Path $tempDir | Out-Null
            try {
                $archivePath = Join-Path $tempDir "asset.zip"
                Invoke-WebRequest -Uri $sourceUrl -OutFile $archivePath
                if ($Asset.archive.format -ne "zip") {
                    throw "Unsupported archive format '$($Asset.archive.format)' for $($Asset.id)."
                }
                $extractDir = Join-Path $tempDir "extracted"
                Expand-Archive -LiteralPath $archivePath -DestinationPath $extractDir -Force
                $member = Get-ChildItem -LiteralPath $extractDir -Recurse -File -Filter $Asset.archive.member_glob | Select-Object -First 1
                if ($null -eq $member) {
                    throw "No archive member matched '$($Asset.archive.member_glob)' for $($Asset.id)."
                }
                Copy-Item -LiteralPath $member.FullName -Destination $target -Force
            } finally {
                Remove-Item -Recurse -Force -LiteralPath $tempDir -ErrorAction SilentlyContinue
            }
        } else {
            Invoke-WebRequest -Uri $sourceUrl -OutFile $target
        }
    } else {
        Write-Host "Already present: $($Asset.local_path)"
    }

    $hash = Get-Sha256 $target
    if ($null -ne $Asset.sha256 -and $Asset.sha256 -ne "") {
        $expected = $Asset.sha256.ToLowerInvariant()
        if ($hash -ne $expected) {
            throw "Checksum failed for $($Asset.local_path): expected $expected, got $hash"
        }
    }

    $sourceName = if ($Asset.PSObject.Properties.Name -contains "source_name") { $Asset.source_name } else { $null }

    return [ordered]@{
        id = $Asset.id
        kind = $Asset.kind
        local_path = $Asset.local_path
        source_name = $sourceName
        url = $sourceUrl
        sha256 = $hash
        expected_sha256 = $Asset.sha256
        size_bytes = (Get-Item -LiteralPath $target).Length
    }
}

function Get-ManifestDirectoryArchive {
    param([Parameter(Mandatory = $true)]$Asset)

    $target = Resolve-RepoPath $Asset.local_path
    $present = (Test-Path -LiteralPath $target) -and `
        ((Get-ChildItem -LiteralPath $target -ErrorAction SilentlyContinue | Measure-Object).Count -gt 0)

    if (-not $present) {
        Write-Host "Downloading $($Asset.id) -> $($Asset.local_path)/"
        $tempDir = Join-Path ([System.IO.Path]::GetTempPath()) ("mite-" + [System.Guid]::NewGuid().ToString("N"))
        New-Item -ItemType Directory -Force -Path $tempDir | Out-Null
        try {
            $archivePath = Join-Path $tempDir "asset.zip"
            Invoke-WebRequest -Uri $Asset.archive.url -OutFile $archivePath
            if ($Asset.archive.format -ne "zip") {
                throw "Unsupported archive format '$($Asset.archive.format)' for $($Asset.id)."
            }
            New-Item -ItemType Directory -Force -Path $target | Out-Null
            Expand-Archive -LiteralPath $archivePath -DestinationPath $target -Force
        } finally {
            Remove-Item -Recurse -Force -LiteralPath $tempDir -ErrorAction SilentlyContinue
        }
    } else {
        Write-Host "Already present: $($Asset.local_path)/"
    }

    return [ordered]@{
        id = $Asset.id
        kind = $Asset.kind
        local_path = $Asset.local_path
        url = $Asset.archive.url
        license = $Asset.license
        file_count = (Get-ChildItem -LiteralPath $target -Recurse -File | Measure-Object).Count
    }
}

function Install-Models {
    $manifestPath = Join-Path $root "model-manifest.json"
    $manifest = Get-Content -Raw -LiteralPath $manifestPath | ConvertFrom-Json
    New-Item -ItemType Directory -Force -Path (Join-Path $root "models") | Out-Null

    $lock = [ordered]@{
        schema = 1
        generated_at = (Get-Date).ToUniversalTime().ToString("o")
        source = $manifest.source
        models = @()
        lexicons = @()
        frequencies = @()
    }

    foreach ($model in $manifest.models) {
        $isOptional = ($model.PSObject.Properties.Name -contains "optional") -and $model.optional
        $isServerModel = $model.id -like "*server*"
        if ($isOptional -and -not ($IncludeServerModels -and $isServerModel)) {
            Write-Host "Skipping optional asset: $($model.local_path)"
            continue
        }
        $lock.models += Get-ManifestAsset $model
    }

    if ($manifest.PSObject.Properties.Name -contains "lexicons") {
        foreach ($lexicon in $manifest.lexicons) {
            $lock.lexicons += Get-ManifestAsset $lexicon
        }
    }

    if ($manifest.PSObject.Properties.Name -contains "frequencies") {
        foreach ($frequency in $manifest.frequencies) {
            $lock.frequencies += Get-ManifestDirectoryArchive $frequency
        }
    }

    $lockPath = Join-Path $root "models/MODELS.lock.json"
    $lock | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $lockPath -Encoding UTF8
    Write-Host "Wrote $lockPath"
}

function Copy-DllsFrom {
    param(
        [Parameter(Mandatory = $true)]
        [string]$SourceDir,
        [Parameter(Mandatory = $true)]
        [string]$Label,
        [Parameter(Mandatory = $true)]
        [string]$RuntimeBin
    )

    if (-not (Test-Path $SourceDir)) {
        throw "$Label DLL directory not found at $SourceDir"
    }

    $files = @(Get-ChildItem -Path $SourceDir -Filter "*.dll" -File -ErrorAction Stop)
    if (-not $files) {
        throw "$Label DLL directory contains no DLLs: $SourceDir"
    }

    foreach ($file in $files) {
        Copy-Item -LiteralPath $file.FullName -Destination $RuntimeBin -Force
    }

    Write-Host "Cached $($files.Count) $Label DLL(s) from $SourceDir"
}

function Copy-DllsFromOptional {
    param(
        [Parameter(Mandatory = $true)]
        [string]$SourceDir,
        [Parameter(Mandatory = $true)]
        [string]$Label,
        [Parameter(Mandatory = $true)]
        [string]$RuntimeBin
    )

    if (Test-Path $SourceDir) {
        Copy-DllsFrom -SourceDir $SourceDir -Label $Label -RuntimeBin $RuntimeBin
    }
}

function Assert-DllsPresent {
    param(
        [Parameter(Mandatory = $true)]
        [string[]]$Names,
        [Parameter(Mandatory = $true)]
        [string]$RuntimeBin
    )

    $missing = @(
        foreach ($name in $Names) {
            if (-not (Test-Path (Join-Path $RuntimeBin $name))) {
                $name
            }
        }
    )

    if ($missing) {
        throw "GPU runtime cache is missing required DLLs: $($missing -join ', ')"
    }
}

function Install-GpuRuntime {
    Assert-Command "uv" "Install uv, then rerun this script: https://docs.astral.sh/uv/getting-started/installation/"
    if (-not $SkipDriverCheck) {
        Assert-NvidiaDriver
    }

    $venv = Join-Path $root ".venv-models"
    $python = Join-Path $venv "Scripts\python.exe"
    $runtimeRoot = Join-Path $root ".gpu-runtime"
    $runtimeBin = Join-Path $runtimeRoot "bin"
    $manifestPath = Join-Path $runtimeRoot "manifest.json"
    $packages = @(
        # ORT 2.0.0-rc.12 imports nvinfer_10.dll directly; TensorRT 11 wheels
        # only ship nvinfer_11.dll, so use the newest compatible 10.x wheel.
        "tensorrt-cu12==10.16.1.11",
        "nvidia-cuda-runtime-cu12==12.9.79",
        "nvidia-cuda-nvrtc-cu12==12.9.86",
        "nvidia-cublas-cu12==12.9.2.10",
        "nvidia-cudnn-cu12==9.23.1.3"
    )

    New-Item -ItemType Directory -Path $runtimeBin -Force | Out-Null
    Get-ChildItem -LiteralPath $runtimeBin -Filter "*.dll" -File -ErrorAction SilentlyContinue |
        Remove-Item -Force

    if (-not (Test-Path $python)) {
        Write-Host "Creating model/runtime venv at $venv ..."
        uv venv --python 3.11 $venv
    }

    Write-Host "Installing GPU runtime wheels..."
    uv pip install --python $python $packages

    $sitePackages = Join-Path $venv "Lib\site-packages"
    $tensorrtLibs = Join-Path $sitePackages "tensorrt_libs"
    $nvidiaRoot = Join-Path $sitePackages "nvidia"

    Copy-DllsFrom -SourceDir $tensorrtLibs -Label "TensorRT" -RuntimeBin $runtimeBin
    Copy-DllsFrom -SourceDir (Join-Path $nvidiaRoot "cuda_runtime\bin") -Label "CUDA runtime" -RuntimeBin $runtimeBin
    Copy-DllsFrom -SourceDir (Join-Path $nvidiaRoot "cuda_nvrtc\bin") -Label "CUDA NVRTC" -RuntimeBin $runtimeBin
    Copy-DllsFrom -SourceDir (Join-Path $nvidiaRoot "cublas\bin") -Label "cuBLAS" -RuntimeBin $runtimeBin
    Copy-DllsFrom -SourceDir (Join-Path $nvidiaRoot "cudnn\bin") -Label "cuDNN" -RuntimeBin $runtimeBin

    $knownNvidiaDirs = @("cuda_runtime", "cuda_nvrtc", "cublas", "cudnn")
    foreach ($dir in Get-ChildItem -Path $nvidiaRoot -Directory -ErrorAction SilentlyContinue) {
        if ($knownNvidiaDirs -contains $dir.Name) {
            continue
        }
        Copy-DllsFromOptional -SourceDir (Join-Path $dir.FullName "bin") -Label $dir.Name -RuntimeBin $runtimeBin
    }

    Assert-DllsPresent -RuntimeBin $runtimeBin -Names @(
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
        created_by = "scripts/bootstrap-dev.ps1"
        runtime_bin = $runtimeBin
        packages = $packages
        dll_count = $cachedDlls.Count
        dlls = $cachedDlls
    }
    $manifest | ConvertTo-Json -Depth 4 | Set-Content -Path $manifestPath -Encoding UTF8
    Write-Host "GPU runtime ready in $runtimeBin"
    Write-Host "Manifest: $manifestPath"
}

function Install-Hooks {
    Assert-Command "git" "Install Git for Windows: https://git-scm.com/download/win"
    $gitRoot = git rev-parse --show-toplevel
    if ($LASTEXITCODE -ne 0) {
        throw "scripts\bootstrap-dev.ps1 must be run from a Git worktree"
    }
    git -C $gitRoot config core.hooksPath .githooks
    Write-Host "Installed local Git hooks from .githooks"
    Write-Host "pre-commit runs: scripts\precommit.ps1"
}

function Install-EvalData {
    Assert-Command "git" "Install Git for Windows: https://git-scm.com/download/win"
    Assert-Command "git-lfs" "Install Git LFS: https://git-lfs.com/"
    git lfs version
    git submodule update --init eval
    git -C eval lfs pull
}

function Assert-CoreDevTools {
    Assert-Command "git" "Install Git for Windows: https://git-scm.com/download/win"
    Assert-Command "rustc" "Install Rust with rustup: https://rustup.rs/"
    Assert-Command "cargo" "Install Rust with rustup: https://rustup.rs/"
    git --version
    rustc --version
    cargo --version
}

if ($ModelsOnly) {
    Invoke-Step "download OCR models, dictionaries, and frequency data" { Install-Models }
    return
}

if ($GpuRuntimeOnly) {
    Invoke-Step "install pinned GPU runtime DLL cache" { Install-GpuRuntime }
    return
}

if ($HooksOnly) {
    Invoke-Step "install local git hooks" { Install-Hooks }
    return
}

if ($EvalDataOnly) {
    Invoke-Step "initialize private eval data submodule" { Install-EvalData }
    return
}

Invoke-Step "check required development tools" {
    Assert-CoreDevTools
    if (-not $SkipGpuRuntime) {
        Assert-Command "uv" "Install uv, then rerun this script: https://docs.astral.sh/uv/getting-started/installation/"
        uv --version
        if (-not $SkipDriverCheck) {
            Assert-NvidiaDriver
        }
    }
}

Invoke-Step "create local directories" {
    New-Item -ItemType Directory -Force -Path "models" | Out-Null
    New-Item -ItemType Directory -Force -Path "cache\engines" | Out-Null
}

if (-not $SkipHooks) {
    Invoke-Step "install local git hooks" { Install-Hooks }
}

if ($IncludeEvalData) {
    Invoke-Step "initialize private eval data submodule" { Install-EvalData }
} else {
    Write-Host ""
    Write-Host "Skipping private eval data. Rerun with -IncludeEvalData or -EvalDataOnly when you need labeled real-image evals."
}

if (-not $SkipModelDownload) {
    Invoke-Step "download OCR models, dictionaries, and frequency data" { Install-Models }
}

if (-not (Test-Path -LiteralPath "mite.toml")) {
    Invoke-Step "write default mite.toml" {
        cargo run -- init-config
    }
}

if (-not $SkipGpuRuntime) {
    Invoke-Step "install pinned GPU runtime DLL cache" { Install-GpuRuntime }
} else {
    Write-Host ""
    Write-Host "Skipping GPU runtime install. The default mite.toml expects NVIDIA TensorRT/CUDA; choose a non-NVIDIA runtime before running doctor/watch."
}

if (-not $SkipBuild) {
    Invoke-Step "build Mite" {
        if ($Release) {
            cargo build --release
        } else {
            cargo build
        }
    }
}

if (-not $SkipDoctor -and -not $SkipGpuRuntime) {
    Invoke-Step "run doctor" {
        cargo run -- doctor
    }
}

Write-Host ""
Write-Host "Mite development setup complete."
Write-Host "Useful next commands:"
Write-Host "  cargo run -- list-windows"
Write-Host "  cargo run -- watch --title `"Target Game`" --auto"
Write-Host "  .\scripts\precommit.ps1"
