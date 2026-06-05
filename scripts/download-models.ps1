Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

$root = Split-Path -Parent $PSScriptRoot
$manifestPath = Join-Path $root "model-manifest.json"
$manifest = Get-Content -Raw -LiteralPath $manifestPath | ConvertFrom-Json

function Resolve-RepoPath {
    param([Parameter(Mandatory = $true)][string]$Path)
    return Join-Path $root ($Path -replace "/", [System.IO.Path]::DirectorySeparatorChar)
}

function Get-Sha256 {
    param([Parameter(Mandatory = $true)][string]$Path)
    return (Get-FileHash -Algorithm SHA256 -LiteralPath $Path).Hash.ToLowerInvariant()
}

New-Item -ItemType Directory -Force -Path (Join-Path $root "models") | Out-Null

function Get-Asset {
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

function Get-DirArchive {
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

$lock = [ordered]@{
    schema = 1
    generated_at = (Get-Date).ToUniversalTime().ToString("o")
    source = $manifest.source
    models = @()
    lexicons = @()
    frequencies = @()
}

foreach ($model in $manifest.models) {
    $lock.models += Get-Asset $model
}

if ($manifest.PSObject.Properties.Name -contains "lexicons") {
    foreach ($lexicon in $manifest.lexicons) {
        $lock.lexicons += Get-Asset $lexicon
    }
}

if ($manifest.PSObject.Properties.Name -contains "frequencies") {
    foreach ($frequency in $manifest.frequencies) {
        $lock.frequencies += Get-DirArchive $frequency
    }
}

$lockPath = Join-Path $root "models/MODELS.lock.json"
$lock | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $lockPath -Encoding UTF8
Write-Host "Wrote $lockPath"
Write-Host "Model setup complete."
