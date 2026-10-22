<#
.SYNOPSIS
    Downloads the pinned third-party assets InTerminal bundles: the static embedding
    model used by semantic search and the llama.cpp CPU server used by the AI
    command generator. Every file is verified against a pinned SHA-256 so a
    tampered or truncated download fails loudly instead of being bundled.

.EXAMPLE
    powershell -NoProfile -ExecutionPolicy Bypass -File scripts\fetch-assets.ps1
#>
[CmdletBinding()]
param(
    [switch]$Force
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue' # Invoke-WebRequest is ~10x slower with the progress bar on 5.1

$resources = Join-Path $PSScriptRoot '..\src-tauri\resources' | Resolve-Path | Select-Object -ExpandProperty Path

# Pinned to an immutable Hugging Face commit so the URLs always serve the same bytes.
$potionRevision = 'bf8b056651a2c21b8d2565580b8569da283cab23'
$potionBase = "https://huggingface.co/minishlab/potion-base-8M/resolve/$potionRevision"
$embeddingFiles = @(
    @{ Name = 'config.json';       Sha256 = '2a6ac0e9aaa356a68a5688070db78fc3a464fefe85d2f06a1905ce3718687553' },
    @{ Name = 'tokenizer.json';    Sha256 = 'e67e803f624fb4d67dea1c730d06e1067e1b14d830e2c2202569e3ef0f70bb50' },
    @{ Name = 'model.safetensors'; Sha256 = 'f65d0f325faadc1e121c319e2faa41170d3fa07d8c89abd48ca5358d9a223de2' }
)

$llamaBuild = 'b11397'
$llamaZip = "llama-$llamaBuild-bin-win-cpu-x64.zip"
$llamaUrl = "https://github.com/ggml-org/llama.cpp/releases/download/$llamaBuild/$llamaZip"
$llamaSha256 = '999ae6b2dbdb6e8fe09e5e20deeb167e5ad7adbd6e80a7366ca8fcf366507779'

$tldrRelease = 'v2.3'
$tldrUrl = "https://github.com/tldr-pages/tldr/releases/download/$tldrRelease/tldr-pages.en.zip"
$tldrSha256 = '80341f9344be09856a0270a90fe38ba8951fd93a4474d18effc27df17525ec4c'

function Get-Sha256([string]$Path) {
    (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Invoke-VerifiedDownload([string]$Url, [string]$Destination, [string]$ExpectedSha256) {
    if ((Test-Path -LiteralPath $Destination) -and -not $Force) {
        if (-not $ExpectedSha256 -or (Get-Sha256 $Destination) -eq $ExpectedSha256) {
            Write-Host "  up to date: $Destination"
            return
        }
    }
    Write-Host "  downloading $Url"
    $partial = "$Destination.partial"
    Invoke-WebRequest -Uri $Url -OutFile $partial -UseBasicParsing
    if ($ExpectedSha256) {
        $actual = Get-Sha256 $partial
        if ($actual -ne $ExpectedSha256) {
            Remove-Item -LiteralPath $partial -Force
            throw "SHA-256 mismatch for $Url`n  expected $ExpectedSha256`n  actual   $actual"
        }
    }
    Move-Item -LiteralPath $partial -Destination $Destination -Force
}

Write-Host 'Embedding model (minishlab/potion-base-8M, MIT):'
$embeddingDir = Join-Path $resources 'embedding'
New-Item -ItemType Directory -Force -Path $embeddingDir | Out-Null
foreach ($file in $embeddingFiles) {
    Invoke-VerifiedDownload "$potionBase/$($file.Name)" (Join-Path $embeddingDir $file.Name) $file.Sha256
}

Write-Host "llama.cpp server ($llamaBuild, MIT):"
$llamaDir = Join-Path $resources 'llama'
$marker = Join-Path $llamaDir ".build-$llamaBuild"
if ((Test-Path -LiteralPath $marker) -and -not $Force) {
    Write-Host "  up to date: $llamaDir"
} else {
    $zipPath = Join-Path ([System.IO.Path]::GetTempPath()) $llamaZip
    Invoke-VerifiedDownload $llamaUrl $zipPath $llamaSha256
    if (Test-Path -LiteralPath $llamaDir) { Remove-Item -LiteralPath $llamaDir -Recurse -Force }
    $extractDir = Join-Path ([System.IO.Path]::GetTempPath()) "interminal-llama-$llamaBuild"
    if (Test-Path -LiteralPath $extractDir) { Remove-Item -LiteralPath $extractDir -Recurse -Force }
    Expand-Archive -LiteralPath $zipPath -DestinationPath $extractDir
    New-Item -ItemType Directory -Force -Path $llamaDir | Out-Null
    # Only the server and the DLLs it loads are needed; the zip also ships ~40 unrelated
    # tools, each as a small .exe stub plus a "<tool>-impl.dll".
    $server = Get-ChildItem -LiteralPath $extractDir -Recurse -Filter 'llama-server.exe' | Select-Object -First 1
    if (-not $server) { throw "llama-server.exe not found in $llamaZip" }
    Copy-Item -LiteralPath $server.FullName -Destination $llamaDir
    $unusedTools = '^llama-(batched-bench|bench|cli|completion|perplexity|quantize)-impl\.dll$'
    Get-ChildItem -LiteralPath $server.DirectoryName -Filter '*.dll' |
        Where-Object { $_.Name -notmatch $unusedTools } |
        Copy-Item -Destination $llamaDir
    Get-ChildItem -LiteralPath $extractDir -Recurse -Filter 'LICENSE*' | Select-Object -First 1 | Copy-Item -Destination (Join-Path $llamaDir 'LICENSE-llama.cpp.txt')
    Remove-Item -LiteralPath $extractDir -Recurse -Force
    New-Item -ItemType File -Path $marker | Out-Null
    Write-Host "  extracted to $llamaDir"
}

Write-Host "tldr-pages command examples ($tldrRelease, CC BY 4.0):"
$tldrDir = Join-Path $resources 'tldr'
$tldrMarker = Join-Path $tldrDir ".release-$tldrRelease"
if ((Test-Path -LiteralPath $tldrMarker) -and -not $Force) {
    Write-Host "  up to date: $tldrDir"
} else {
    $zipPath = Join-Path ([System.IO.Path]::GetTempPath()) "tldr-pages-$tldrRelease.en.zip"
    Invoke-VerifiedDownload $tldrUrl $zipPath $tldrSha256
    if (Test-Path -LiteralPath $tldrDir) { Remove-Item -LiteralPath $tldrDir -Recurse -Force }
    New-Item -ItemType Directory -Force -Path $tldrDir | Out-Null
    # Only the platform-independent and Windows pages apply, and one file loads
    # far faster than thousands of tiny ones. Each page is preceded by a
    # "%%% <platform>/<page>.md" line.
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $zip = [System.IO.Compression.ZipFile]::OpenRead($zipPath)
    try {
        $builder = New-Object System.Text.StringBuilder
        $pageCount = 0
        foreach ($entry in $zip.Entries) {
            $isPage = $entry.FullName -match '^(common|windows)/[^/]+\.md$'
            if ($entry.FullName -eq 'LICENSE.md') {
                $reader = New-Object System.IO.StreamReader($entry.Open())
                try { [System.IO.File]::WriteAllText((Join-Path $tldrDir 'LICENSE.md'), $reader.ReadToEnd()) } finally { $reader.Dispose() }
            } elseif ($isPage) {
                $reader = New-Object System.IO.StreamReader($entry.Open())
                try {
                    [void]$builder.Append("%%% $($entry.FullName)`n").Append($reader.ReadToEnd().Replace("`r`n", "`n")).Append("`n")
                    $pageCount++
                } finally { $reader.Dispose() }
            }
        }
        [System.IO.File]::WriteAllText((Join-Path $tldrDir 'pages.md'), $builder.ToString(), (New-Object System.Text.UTF8Encoding($false)))
    } finally {
        $zip.Dispose()
    }
    New-Item -ItemType File -Path $tldrMarker | Out-Null
    Write-Host "  bundled $pageCount pages into $tldrDir\pages.md"
}

Write-Host 'Done.'
