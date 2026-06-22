param(
    [switch]$ModelsOnly,
    [switch]$HooksOnly,
    [switch]$EvalDataOnly,
    [switch]$SkipModelDownload,
    [switch]$IncludeServerModels,
    [switch]$SkipBuild,
    [switch]$SkipDoctor,
    [switch]$SkipHooks,
    [switch]$IncludeEvalData,
    [switch]$Release
)

# This script does not install the NVIDIA GPU runtime (TensorRT, CUDA, cuDNN,
# NVRTC, cuBLAS). Mite never downloads, hosts, bundles, or installs NVIDIA
# binaries, and that applies to developer tooling too. Install the runtime
# yourself from NVIDIA (the same components, pinned to the same majors, that the
# desktop app guides end users to install) and make it discoverable on PATH;
# `cargo run -- doctor` reports the detected tier. See docs\local-windows.md.

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$focusedModes = @(
    @($ModelsOnly, $HooksOnly, $EvalDataOnly) |
        Where-Object { $_ }
)
if ($focusedModes.Count -gt 1) {
    throw "Choose at most one focused mode: -ModelsOnly, -HooksOnly, or -EvalDataOnly."
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

if (-not $SkipBuild) {
    Invoke-Step "build Mite" {
        if ($Release) {
            cargo build --release
        } else {
            cargo build
        }
    }
}

if (-not $SkipDoctor) {
    Invoke-Step "run doctor" {
        cargo run -- doctor
    }
}

Write-Host ""
Write-Host "Mite development setup complete."
Write-Host ""
Write-Host "The default mite.toml expects the NVIDIA TensorRT/CUDA runtime, which this"
Write-Host "script does not install. If doctor reports the CPU tier and you have an"
Write-Host "NVIDIA GPU, install the runtime yourself and re-run doctor (see"
Write-Host "docs\local-windows.md):"
Write-Host "  - the CUDA Toolkit and cuDNN from NVIDIA (they add themselves to PATH), or"
Write-Host "  - the pinned pip wheels (tensorrt-cu12-libs, nvidia-cuda-runtime-cu12,"
Write-Host "    nvidia-cuda-nvrtc-cu12, nvidia-cublas-cu12, nvidia-cudnn-cu12) from"
Write-Host "    --extra-index-url https://pypi.nvidia.com, with their wheel bin"
Write-Host "    directories on PATH."
Write-Host "Without it, choose a non-NVIDIA runtime in mite.toml before running watch."
Write-Host ""
Write-Host "Useful next commands:"
Write-Host "  cargo run -- doctor"
Write-Host "  cargo run -- list-windows"
Write-Host "  cargo run -- watch --title `"Target Game`" --auto"
Write-Host "  .\scripts\precommit.ps1"
