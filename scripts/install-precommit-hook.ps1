Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$gitRoot = git rev-parse --show-toplevel
if ($LASTEXITCODE -ne 0) {
    throw "scripts\install-precommit-hook.ps1 must be run from a Git worktree"
}
Set-Location $gitRoot

git config core.hooksPath .githooks
Write-Host "installed local Git hooks from .githooks"
Write-Host "pre-commit runs: scripts\precommit.ps1"
