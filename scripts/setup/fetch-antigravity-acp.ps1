# SPDX-License-Identifier: MPL-2.0

# Place Google's official Antigravity ACP server, the managed route a Koru chat
# uses for Antigravity (AGENTUX I9), and verify it against the pins below.
#
# Why this exists: the server is not the `agy` CLI and is not on npm. Google
# publishes it as a signed zip on dl.google.com, listed in the ACP registry
# (agentclientprotocol/registry, antigravity-acp). Koru's route is pinned to the
# exact executable - the server prints no version, so its SHA-256 is the pin in
# crates/koru-session-runtime/src/providers/acp/routes.rs - so the download is a
# setup step, never something the app does at runtime. The npm package called
# antigravity-acp is a third-party wrapper and is not this.
#
#   scripts\setup\fetch-antigravity-acp.ps1           download, verify, put on PATH
#   scripts\setup\fetch-antigravity-acp.ps1 -Verify   check what is already here
#   scripts\setup\fetch-antigravity-acp.ps1 -SignIn   one-time Google sign-in for the
#                                               server (opens a browser)
#
# Where: %LOCALAPPDATA%\Koru\AgentTools\agy-acp-server-<version>, beside the
# checksum-verified ConfigTools that setup-config places, and like them its
# folder is added to the user PATH. Koru finds the server on PATH as
# `agy_acp_server`, the same rule as every other managed agent, so a Koru that
# was already running needs a restart to see it.
#
# The server keeps its own sign-in, separate from `agy`'s: until -SignIn has
# run once, Koru shows Antigravity as signed out. -SignIn starts the server and
# asks it to log in with a personal Google account; the server opens the
# browser and records the choice in %USERPROFILE%\.gemini\antigravity-acp\settings.json.
#
# Windows x64 only: that is the one build qualified.
#
# ASCII only. Windows PowerShell 5.1 reads a BOM-less script as ANSI, and a
# UTF-8 dash here arrives as three cp1252 characters, one of which is a curly
# quote that silently opens a string and breaks the parse pages later.

[CmdletBinding()]
param(
    [switch]$Verify,
    [switch]$SignIn
)

$ErrorActionPreference = 'Stop'

$Version = '1.2.1'
$Archive = [pscustomobject]@{
    Url  = "https://dl.google.com/agy-extensions/releases/windows/agy-acp-server-$Version-windows-x86_64.zip"
    Sha  = '9b82493819bc14613baa76264d55ad307ddd8ab4a8d6e110edb32da35498c07b'
    Size = 124869770
}
# The server starts localharness_external.exe from its own folder, so both are pinned.
$Members = @(
    [pscustomobject]@{ Name = 'agy_acp_server.exe';        Sha = '04fd51b5c6dfb6d11e649c2cfc61a64ca31d9a4a58a9a3e5c93db934cd7a9695' },
    [pscustomobject]@{ Name = 'localharness_external.exe'; Sha = 'd9283bd0505014ca44f0e02259b78209c13b2426c9780bd43b7779f8eafe438f' }
)
$Parent = Join-Path $env:LOCALAPPDATA 'Koru\AgentTools'
$Dest = Join-Path $Parent "agy-acp-server-$Version"

function Test-Members([string]$Folder) {
    $ok = $true
    foreach ($member in $Members) {
        $path = Join-Path $Folder $member.Name
        if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
            Write-Host "[antigravity] $($member.Name) is missing from $Folder"
            $ok = $false
            continue
        }
        $have = (Get-FileHash -Algorithm SHA256 -LiteralPath $path).Hash.ToLowerInvariant()
        if ($have -ne $member.Sha) {
            Write-Host "[antigravity] $($member.Name) does NOT match the pinned sha256"
            Write-Host "[antigravity]   want $($member.Sha)"
            Write-Host "[antigravity]   have $have"
            $ok = $false
        }
    }
    return $ok
}

# The same rule setup-config's Add-ConfigToolPath follows: this shell at once,
# the user PATH for every new one.
function Add-ToolPath([string]$Directory) {
    if (($env:PATH -split ';') -notcontains $Directory) { $env:PATH = "$Directory;$env:PATH" }
    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    if (($userPath -split ';') -notcontains $Directory) {
        [Environment]::SetEnvironmentVariable('Path', ($Directory + ';' + $userPath).TrimEnd(';'), 'User')
        Write-Host "[antigravity] added $Directory to your user PATH; restart Koru to pick it up"
    }
}

function Place-Server {
    if (Test-Path -LiteralPath $Dest) {
        if (Test-Members $Dest) {
            Write-Host "[antigravity] $Dest is already here and verified"
            return $true
        }
        Write-Host "[antigravity] refusing to use $Dest; delete it to re-fetch"
        return $false
    }
    if ($Verify) {
        Write-Host "[antigravity] $Dest is not here"
        return $false
    }
    New-Item -ItemType Directory -Force -Path $Parent | Out-Null
    # Verified into place, never in place: an interrupted download or a
    # mismatched member must not leave a folder the PATH already points at.
    $temporary = Join-Path $Parent ('download-' + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $temporary | Out-Null
    try {
        $zip = Join-Path $temporary 'server.zip'
        Write-Host "[antigravity] downloading $($Archive.Url)"
        # Invoke-WebRequest rather than curl.exe: a native command writing
        # progress to stderr is a terminating error under 'Stop'.
        $ProgressPreference = 'SilentlyContinue'
        Invoke-WebRequest -Uri $Archive.Url -OutFile $zip -UseBasicParsing
        $size = (Get-Item -LiteralPath $zip).Length
        if ($size -ne $Archive.Size) {
            Write-Host "[antigravity] the archive is $size bytes and the pin says $($Archive.Size)"
            return $false
        }
        $have = (Get-FileHash -Algorithm SHA256 -LiteralPath $zip).Hash.ToLowerInvariant()
        if ($have -ne $Archive.Sha) {
            Write-Host "[antigravity] the archive does not match the pinned sha256"
            Write-Host "[antigravity]   want $($Archive.Sha)"
            Write-Host "[antigravity]   have $have"
            return $false
        }
        $expanded = Join-Path $temporary 'expanded'
        Expand-Archive -LiteralPath $zip -DestinationPath $expanded
        if (-not (Test-Members $expanded)) { return $false }
        Move-Item -LiteralPath $expanded -Destination $Dest
        Write-Host "[antigravity] placed $Dest"
        return $true
    } finally {
        $resolved = [IO.Path]::GetFullPath($temporary)
        if ([IO.Path]::GetDirectoryName($resolved) -ne [IO.Path]::GetFullPath($Parent) -or
            [IO.Path]::GetFileName($resolved) -notmatch '^download-[a-f0-9]{32}$') { throw 'Download cleanup path refused.' }
        Remove-Item -LiteralPath $resolved -Recurse -Force
    }
}

# One JSON-RPC request on the server's stdin; the answer with the same id, or
# $null after the timeout. Notifications and other frames are skipped.
function Invoke-Rpc($Process, [int]$Id, [string]$Method, $Params, [int]$TimeoutMs) {
    $line = @{ jsonrpc = '2.0'; id = $Id; method = $Method; params = $Params } | ConvertTo-Json -Compress -Depth 8
    $Process.StandardInput.WriteLine($line)
    $Process.StandardInput.Flush()
    $deadline = [DateTime]::UtcNow.AddMilliseconds($TimeoutMs)
    while ([DateTime]::UtcNow -lt $deadline) {
        $read = $Process.StandardOutput.ReadLineAsync()
        $left = [int][Math]::Max(1, ($deadline - [DateTime]::UtcNow).TotalMilliseconds)
        if (-not $read.Wait($left)) { return $null }
        if ($null -eq $read.Result) { return $null }
        $frame = $read.Result | ConvertFrom-Json
        if ($frame.id -eq $Id -and $null -eq $frame.method) { return $frame }
    }
    return $null
}

function Invoke-SignIn {
    $server = Join-Path $Dest 'agy_acp_server.exe'
    $start = New-Object System.Diagnostics.ProcessStartInfo
    $start.FileName = $server
    $start.UseShellExecute = $false
    $start.RedirectStandardInput = $true
    $start.RedirectStandardOutput = $true
    # Stderr stays on this console: if no browser opens, the server prints
    # the sign-in link there.
    $start.RedirectStandardError = $false
    $start.WorkingDirectory = $env:USERPROFILE
    $process = [System.Diagnostics.Process]::Start($start)
    try {
        Write-Host '[antigravity] starting the server (about ten seconds)...'
        $init = Invoke-Rpc $process 1 'initialize' @{ protocolVersion = 1; clientCapabilities = @{} } 60000
        if ($null -eq $init -or $null -eq $init.result) { Write-Host '[antigravity] the server did not answer initialize'; return $false }
        Write-Host "[antigravity] server $($init.result.agentInfo.name) $($init.result.agentInfo.version)"
        Write-Host '[antigravity] signing in with your Google account; finish it in the browser that opens...'
        $auth = Invoke-Rpc $process 2 'authenticate' @{ methodId = 'oauth-personal' } 300000
        if ($null -eq $auth) { Write-Host '[antigravity] no answer within five minutes'; return $false }
        if ($null -ne $auth.error) { Write-Host "[antigravity] sign-in failed: $($auth.error.message)"; return $false }
        $session = Invoke-Rpc $process 3 'session/new' @{ cwd = $env:USERPROFILE; mcpServers = @() } 60000
        if ($null -eq $session -or $null -ne $session.error) { Write-Host '[antigravity] signed in, but a session did not start'; return $false }
        $models = @($session.result.configOptions | Where-Object { $_.id -eq 'model' } | ForEach-Object { $_.options })
        Write-Host "[antigravity] signed in; the server offers $($models.Count) models"
        return $true
    } finally {
        # The server never exits on EOF; end its whole tree.
        & taskkill.exe /T /F /PID $process.Id 2>&1 | Out-Null
    }
}

if (-not (Place-Server)) { exit 1 }
if (-not $Verify) { Add-ToolPath $Dest }
if ($SignIn) {
    if (-not (Invoke-SignIn)) { exit 1 }
}
exit 0
