# SPDX-License-Identifier: MPL-2.0

param(
    [string]$Codex = "codex",
    [Parameter(Mandatory = $true)]
    [string]$Out,
    [string]$Cwd = (Get-Location).Path,
    [string]$Prompt = "Reply with the single word OK.",
    [int]$TurnTimeoutSeconds = 120
)

# Drives one Codex app-server session the way the Koru adapter does and
# writes the RAW transcript under $Out. The raw files carry the account's
# codex home, platform strings, model ids, thread/turn/item ids and the
# model's own words: they are evidence for a person sanitizing a fixture set
# by hand and must never be committed. The sanitized set lives under
# tests/fixtures/codex-app-server/<version>/ and is what the offline replay
# reads. Sanitizing replaces the codex home and account paths, platform
# strings, model ids and thread/turn/item ids, and drops the model's own
# words; a version's README there says what its set replaced.

$ErrorActionPreference = "Stop"
if (Test-Path -LiteralPath $Out) {
    if (Get-ChildItem -LiteralPath $Out -Force | Select-Object -First 1) {
        throw "Output directory must be empty: $Out"
    }
} else {
    New-Item -ItemType Directory -Path $Out | Out-Null
}

$version = (& $Codex --version) -join " "
if ($LASTEXITCODE -ne 0) { throw "Codex version probe failed" }
Set-Content -LiteralPath (Join-Path $Out "version.txt") -Value $version -Encoding utf8

$startInfo = New-Object System.Diagnostics.ProcessStartInfo
$startInfo.FileName = "cmd.exe"
$startInfo.Arguments = "/D /S /C `"$Codex app-server --stdio`""
$startInfo.WorkingDirectory = $Cwd
$startInfo.RedirectStandardInput = $true
$startInfo.RedirectStandardOutput = $true
$startInfo.RedirectStandardError = $true
$startInfo.UseShellExecute = $false
$startInfo.CreateNoWindow = $true
$process = [System.Diagnostics.Process]::Start($startInfo)
$stdin = $process.StandardInput
$stdout = $process.StandardOutput
$transcript = New-Object System.Collections.Generic.List[string]

function Send-Line([string]$line) {
    $transcript.Add("client: " + $line)
    $stdin.WriteLine($line)
    $stdin.Flush()
}

function Read-Until([scriptblock]$done, [int]$timeoutSeconds) {
    $deadline = (Get-Date).AddSeconds($timeoutSeconds)
    while ((Get-Date) -lt $deadline) {
        $task = $stdout.ReadLineAsync()
        while (-not $task.Wait(200)) {
            if ((Get-Date) -ge $deadline) { return $false }
        }
        $line = $task.Result
        if ($null -eq $line) { return $false }
        $transcript.Add("server: " + $line)
        try { $message = $line | ConvertFrom-Json } catch { continue }
        if (& $done $message) { return $true }
    }
    return $false
}

Send-Line '{"method":"initialize","id":1,"params":{"clientInfo":{"name":"koru_terminal","title":"Koru Terminal","version":"capture"},"capabilities":{"experimentalApi":false}}}'
if (-not (Read-Until { param($m) $m.id -eq 1 } 30)) { throw "no initialize response" }
Send-Line '{"method":"initialized","params":{}}'
Send-Line '{"method":"model/list","id":2,"params":{"limit":100}}'
if (-not (Read-Until { param($m) $m.id -eq 2 } 30)) { throw "no model/list response" }
$models = ($transcript | Where-Object { $_ -like 'server: {"id":2,*' } | Select-Object -Last 1).Substring(8) | ConvertFrom-Json
$default = ($models.result.data | Where-Object { $_.isDefault -eq $true } | Select-Object -First 1).model
$cwdJson = $Cwd | ConvertTo-Json
Send-Line ('{"method":"thread/start","id":3,"params":{"cwd":' + $cwdJson + ',"ephemeral":true,"model":"' + $default + '"}}')
if (-not (Read-Until { param($m) $m.id -eq 3 } 30)) { throw "no thread/start response" }
$thread = ($transcript | Where-Object { $_ -like 'server: {"id":3,*' } | Select-Object -Last 1).Substring(8) | ConvertFrom-Json
$threadId = $thread.result.thread.id
$promptJson = $Prompt | ConvertTo-Json
Send-Line ('{"method":"turn/start","id":4,"params":{"threadId":"' + $threadId + '","input":[{"type":"text","text":' + $promptJson + '}]}}')
if (-not (Read-Until { param($m) $m.method -eq "turn/completed" } $TurnTimeoutSeconds)) { Write-Warning "turn did not complete within the timeout" }

# A second turn, interrupted as soon as it has started.
Send-Line ('{"method":"turn/start","id":5,"params":{"threadId":"' + $threadId + '","input":[{"type":"text","text":"Count slowly from one to one hundred, one number per line."}]}}')
if (Read-Until { param($m) $m.id -eq 5 } 30) {
    $turn = ($transcript | Where-Object { $_ -like 'server: {"id":5,*' } | Select-Object -Last 1).Substring(8) | ConvertFrom-Json
    $turnId = $turn.result.turn.id
    Start-Sleep -Seconds 2
    Send-Line ('{"method":"turn/interrupt","id":6,"params":{"threadId":"' + $threadId + '","turnId":"' + $turnId + '"}}')
    if (-not (Read-Until { param($m) $m.method -eq "turn/completed" } 60)) { Write-Warning "interrupted turn did not complete" }
}

$stdin.Close()
if (-not $process.WaitForExit(5000)) { $process.Kill() }
Set-Content -LiteralPath (Join-Path $Out "transcript.raw.txt") -Value ($transcript -join "`n") -Encoding utf8
$methods = $transcript | Where-Object { $_ -like 'server: *' } | ForEach-Object {
    try { ($_.Substring(8) | ConvertFrom-Json).method } catch { $null }
} | Where-Object { $_ } | Sort-Object -Unique
Set-Content -LiteralPath (Join-Path $Out "server-methods.txt") -Value ($methods -join "`n") -Encoding utf8
Write-Output "version=$version"
Write-Output "server lines=$(($transcript | Where-Object { $_ -like 'server: *' }).Count)"
Write-Output "server methods:"
$methods | ForEach-Object { Write-Output "  $_" }
Write-Output "raw transcript: $(Join-Path $Out 'transcript.raw.txt') (never commit it)"
