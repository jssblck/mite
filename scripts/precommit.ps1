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
    $global:LASTEXITCODE = 0
    & $Body
    if ($LASTEXITCODE -ne 0) {
        throw "$Name failed with exit code $LASTEXITCODE"
    }
}

# Scope the checks to what actually changed. The marketing site under site\ is an
# independent Node project that the Rust crate does not depend on (and vice
# versa), so a site-only commit should not pay for the full Rust suite, and a
# Rust-only commit should not need Node installed. When the staged list cannot be
# read (e.g. the script is run by hand outside a commit), run everything.
$staged = @()
try {
    $staged = @(& git diff --cached --name-only --diff-filter=ACMR 2>$null |
        Where-Object { $_ })
}
catch {
    $staged = @()
}

$hasStaged = $staged.Count -gt 0
$siteStaged = @($staged | Where-Object { $_ -like "site/*" })
$appStaged = @($staged | Where-Object { $_ -like "app/*" })
$nonSiteStaged = @($staged | Where-Object { $_ -notlike "site/*" })

# Run the Rust checks unless the commit is exclusively site\** changes.
$runRust = (-not $hasStaged) -or ($nonSiteStaged.Count -gt 0)
# Run the site checks whenever site\** changed (or when scope is unknown).
$runSite = (-not $hasStaged) -or ($siteStaged.Count -gt 0)
# Run the app frontend tests whenever app\** changed (or when scope is
# unknown). The app's Rust crate is checked by CI's `app` job instead;
# building the Tauri dependency tree on every commit is too slow for a hook.
$runApp = (-not $hasStaged) -or ($appStaged.Count -gt 0)

if ($runRust) {
    Invoke-Step "cargo fmt --check" {
        cargo fmt --check
    }

    Invoke-Step "nudge check" {
        $nudgePaths = @("Cargo.toml", "src", "docs", "examples", "build.rs")
        if (Test-Path -LiteralPath (Join-Path $root "tests")) {
            $nudgePaths += "tests"
        }
        nudge check @nudgePaths
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
}
else {
    Write-Host ""
    Write-Host "==> rust checks skipped (commit only touches site\**)"
}

if ($runSite) {
    $siteRoot = Join-Path $root "site"
    if (-not (Get-Command npm -ErrorAction SilentlyContinue)) {
        Write-Host ""
        Write-Host "==> site checks skipped (npm not found on PATH)"
    }
    elseif (-not (Test-Path -LiteralPath (Join-Path $siteRoot "node_modules"))) {
        Write-Host ""
        Write-Host "==> site checks skipped (run 'npm install' in site\ to enable them)"
    }
    else {
        Push-Location $siteRoot
        try {
            Invoke-Step "site: astro check" {
                npm run check
            }
            Invoke-Step "site: vitest run" {
                npm test
            }
        }
        finally {
            Pop-Location
        }
    }
}
elseif ($hasStaged) {
    Write-Host ""
    Write-Host "==> site checks skipped (no site\** changes staged)"
}

if ($runApp) {
    $appRoot = Join-Path $root "app"
    if (-not (Get-Command bun -ErrorAction SilentlyContinue)) {
        Write-Host ""
        Write-Host "==> app checks skipped (bun not found on PATH)"
    }
    elseif (-not (Test-Path -LiteralPath (Join-Path $appRoot "node_modules"))) {
        Write-Host ""
        Write-Host "==> app checks skipped (run 'bun install' in app\ to enable them)"
    }
    else {
        Push-Location $appRoot
        try {
            Invoke-Step "app: vitest run" {
                bun run test
            }
        }
        finally {
            Pop-Location
        }
    }
}
elseif ($hasStaged) {
    Write-Host ""
    Write-Host "==> app checks skipped (no app\** changes staged)"
}

Write-Host ""
Write-Host "precommit checks passed"
