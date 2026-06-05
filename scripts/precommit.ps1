param(
    [switch]$IncludeEval
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

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

Invoke-Step "cargo fmt --check" {
    cargo fmt --check
}

Invoke-Step "cargo test" {
    cargo test
}

Invoke-Step "cargo clippy --all-targets -- -D warnings" {
    cargo clippy --all-targets -- -D warnings
}

if ($IncludeEval) {
    $evalRoot = Join-Path $root "eval"
    $evalFiles = @()
    if (Test-Path -LiteralPath $evalRoot) {
        $evalFiles = @(Get-ChildItem -LiteralPath $evalRoot -Recurse -Filter "eval.json" -File)
    }

    if ($evalFiles.Count -eq 0) {
        Write-Host ""
        Write-Host "==> real-image evals"
        Write-Host "no eval\**\eval.json labels found"
    }

    foreach ($evalFile in $evalFiles) {
        $image = Join-Path $evalFile.DirectoryName "underlying.png"
        Invoke-Step "cargo run -- eval --image $image --labels $($evalFile.FullName) --out target\eval\$($evalFile.Directory.Name).json" {
            cargo run -- eval --image $image --labels $evalFile.FullName --out "target\eval\$($evalFile.Directory.Name).json"
        }
    }
}

Write-Host ""
Write-Host "precommit checks passed"
