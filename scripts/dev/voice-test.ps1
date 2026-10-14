# SPDX-License-Identifier: MPL-2.0

# Launch one coherent, isolated worktree stack against the locally served
# Compact and High Accuracy catalog.
[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$Repo = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$Target = Join-Path $Repo 'target'
$Root = Join-Path $Target 'voice-test-root'
$Common = (& git -C $Repo rev-parse --path-format=absolute --git-common-dir).Trim()
if ($LASTEXITCODE -ne 0) { throw 'the main checkout could not be resolved' }
$MainRepo = Split-Path -Parent $Common
$Models = Join-Path $MainRepo 'dist\voice-models'
$Serve = Join-Path $Models 'serve'
$Catalog = Join-Path $Serve 'voice\catalog-v1.json'

Push-Location $Repo
try {
    foreach ($model in @('nemotron-3.5-streaming-0.6b-160ms', 'whisper-base')) {
        & (Join-Path $Repo 'koru.bat') voice-models fetch --work $Models --only $model
        if ($LASTEXITCODE -ne 0) { throw "the local $model archive could not be prepared" }
    }
    & (Join-Path $Repo 'koru.bat') voice-models local --work $Models
    if ($LASTEXITCODE -ne 0) { throw 'the local voice catalog could not be prepared' }
    if (-not (Test-Path -LiteralPath $Catalog)) {
        throw "the local catalog was not written at $Catalog"
    }

    $server = $null
    try {
        try {
            Invoke-WebRequest -UseBasicParsing -TimeoutSec 1 -Uri 'http://127.0.0.1:8787/voice/catalog-v1.json' | Out-Null
        } catch {
            $python = (Get-Command python -ErrorAction Stop).Source
            $server = Start-Process -FilePath $python -ArgumentList @(
                '-m', 'http.server', '8787', '--bind', '127.0.0.1', '--directory', $Serve
            ) -WindowStyle Hidden -PassThru
        }
        $env:KORU_VOICE_CATALOG = $Catalog
        $env:KORU_VOICE_MODELS = Join-Path $Models 'installed'
        $env:KORU_ROOT = $Root
        Write-Host "[Koru] shared voice cache: $Models"
        Write-Host "[Koru] voice test root: $Root"
        Write-Host '[Koru] Compact = Whisper; High Accuracy = Nemotron'
        & (Join-Path $Repo 'koru.bat') run-all
        exit $LASTEXITCODE
    } finally {
        if ($null -ne $server -and -not $server.HasExited) {
            Stop-Process -Id $server.Id
        }
    }
} finally {
    Pop-Location
}
