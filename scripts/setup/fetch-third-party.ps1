# SPDX-License-Identifier: MPL-2.0

# Fetch the third-party code Koru builds with but does not commit: the crates
# it builds from patched source, and a few upstream files used as they are.
#
# The workspace names the crates by path (Cargo.toml) and koru-blocks compiles
# a fetched grammar in, so every cargo command needs them on disk: koru.bat
# runs this before every verb that reaches cargo. The Android build's Gradle
# wrapper is fetched too. Nothing here is committed but the pins and the
# patches; third-party/README.md says why, and how to change one.
#
#   scripts\setup\fetch-third-party.ps1
#   scripts\setup\fetch-third-party.ps1 -UpdatePatch <name>
#
# For each line of third-party\fetch.manifest:
#
#   crate <name> <version> <crate sha256> <patch sha256>
#     download the .crate from crates.io, refuse it or the patch unless each
#     matches its pin, unpack, apply third-party\patches\<name>.patch with
#     `git apply`, and move the result into third-party\crates\<name>\.
#   file <path> <sha256> <url> [exec] [crlf]
#     download <url>, refuse it unless it matches, and place it at <path>
#     (relative to the root); `crlf` gives it CRLF line endings afterwards
#     (a batch file), `exec` marks it executable where that means anything.
#
# An entry whose stamp already matches its line is left alone, so a second run
# touches nothing and local edits survive until the pin changes.
#
# -UpdatePatch rewrites third-party\patches\<name>.patch from your edits to
# third-party\crates\<name>\ and prints the sha256 to pin in fetch.manifest.
#
# Keep in step with scripts/setup/fetch-third-party.sh.
# ASCII only (Windows PowerShell 5.1 reads a BOM-less script as ANSI).

[CmdletBinding()]
param([string]$UpdatePatch)

$ErrorActionPreference = 'Stop'

$Root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$ThirdParty = Join-Path $Root 'third-party'
$Manifest = Join-Path $ThirdParty 'fetch.manifest'
$Crates = Join-Path $ThirdParty 'crates'
$Stamp = Join-Path $Crates '.fetched'

# The whole manifest, byte for byte, is the stamp of a complete fetch.
function Test-SameBytes([string]$A, [string]$B) {
    if (-not (Test-Path -LiteralPath $B)) { return $false }
    $x = [System.IO.File]::ReadAllBytes($A)
    $y = [System.IO.File]::ReadAllBytes($B)
    if ($x.Length -ne $y.Length) { return $false }
    for ($i = 0; $i -lt $x.Length; $i++) { if ($x[$i] -ne $y[$i]) { return $false } }
    return $true
}

if (-not $UpdatePatch -and (Test-SameBytes $Manifest $Stamp)) { exit 0 }

function Get-Sha256([string]$Path) {
    (Get-FileHash -Algorithm SHA256 -LiteralPath $Path).Hash.ToLowerInvariant()
}

# Native commands under 'Stop' turn any stderr line into a terminating error,
# so they run under 'Continue' and are judged by their exit code alone.
function Invoke-Native([string]$Exe, [string[]]$Arguments) {
    $saved = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        $out = & $Exe @Arguments 2>&1 | Out-String
        return @{ Code = $LASTEXITCODE; Out = $out.Trim() }
    } finally {
        $ErrorActionPreference = $saved
    }
}

# Verified into place, never in place: an interrupted download must not leave
# a file that a later run takes for the real one. Returns $false on a mismatch.
function Get-Verified([string]$Url, [string]$Sha, [string]$Out, [string]$What) {
    $part = "$Out.part"
    Write-Host "[third-party] downloading $What"
    $ProgressPreference = 'SilentlyContinue'
    Invoke-WebRequest -UseBasicParsing -Uri $Url -OutFile $part
    $have = Get-Sha256 $part
    if ($have -ne $Sha) {
        Remove-Item -Force -LiteralPath $part
        Write-Host "[third-party] $What does not match its pin in fetch.manifest"
        Write-Host "[third-party]   pin  $Sha"
        Write-Host "[third-party]   got  $have"
        return $false
    }
    Move-Item -Force -LiteralPath $part -Destination $Out
    return $true
}

# Windows' own bsdtar, by path. A GNU tar earlier on PATH (Git's, under Git
# Bash) reads `D:\...` as a remote host and fails.
$Tar = Join-Path $env:SystemRoot 'System32\tar.exe'
if (-not (Test-Path -LiteralPath $Tar)) { $Tar = 'tar' }

$Downloads = Join-Path $Crates '.download'
$Marks = Join-Path $Crates '.files'

if ($UpdatePatch) {
    $entry = Get-Content -LiteralPath $Manifest |
        Where-Object { $f = $_.Trim() -split '\s+'; $f[0] -eq 'crate' -and $f[1] -eq $UpdatePatch }
    if (-not $entry) { throw "[third-party] no crate $UpdatePatch in third-party\fetch.manifest" }
    $version = ($entry.Trim() -split '\s+')[2]
    $edited = Join-Path $Crates $UpdatePatch
    $crate = Join-Path $Downloads "$UpdatePatch-$version.crate"
    if (-not (Test-Path -LiteralPath $edited) -or -not (Test-Path -LiteralPath $crate)) {
        throw "[third-party] fetch first: third-party\crates\$UpdatePatch and its .crate are not both here"
    }
    # Outside the checkout, so this repository's .gitattributes cannot rewrite
    # line endings in upstream's files on their way into the patch.
    $work = Join-Path ([System.IO.Path]::GetTempPath()) "koru-third-party-$PID"
    if (Test-Path -LiteralPath $work) { Remove-Item -Recurse -Force -LiteralPath $work }
    New-Item -ItemType Directory -Force -Path $work | Out-Null
    $r = Invoke-Native $Tar @('-xzf', $crate, '-C', $work)
    if ($r.Code -ne 0) { throw "[third-party] could not unpack ${crate}: $($r.Out)" }
    Rename-Item -LiteralPath (Join-Path $work "$UpdatePatch-$version") 'a'
    Copy-Item -Recurse -LiteralPath $edited -Destination (Join-Path $work 'b')
    Remove-Item -Force -ErrorAction SilentlyContinue -LiteralPath (Join-Path $work 'b\.koru-fetched')
    $patch = Join-Path $ThirdParty "patches\$UpdatePatch.patch"
    # git writes the file itself: piped through Windows PowerShell, the diff
    # would be re-decoded in the console code page and re-joined with CRLF.
    # `git diff --no-index` exits 1 when the trees differ, which they must.
    Push-Location $work
    try {
        $r = Invoke-Native 'git' @('diff', '--no-index', '--no-prefix', "--output=$patch", 'a', 'b')
    } finally {
        Pop-Location
    }
    Remove-Item -Recurse -Force -LiteralPath $work
    if ($r.Code -gt 1) { throw "[third-party] git diff failed: $($r.Out)" }
    Write-Host "[third-party] wrote third-party\patches\$UpdatePatch.patch"
    Write-Host "[third-party] pin it in fetch.manifest: $(Get-Sha256 $patch)"
    exit 0
}

$Staging = Join-Path $Crates '.staging'
New-Item -ItemType Directory -Force -Path $Downloads, $Marks | Out-Null

function Install-Crate([string]$Line, [string[]]$Fields) {
    $name, $version, $crateSha, $patchSha = $Fields
    $dest = Join-Path $Crates $name
    $mark = Join-Path $dest '.koru-fetched'
    if ((Test-Path -LiteralPath $mark) -and ((Get-Content -Raw -LiteralPath $mark).Trim() -eq $Line)) {
        return $true
    }

    $patch = Join-Path $ThirdParty "patches\$name.patch"
    if (-not (Test-Path -LiteralPath $patch)) {
        Write-Host "[third-party] third-party\patches\$name.patch is missing"
        return $false
    }
    $have = Get-Sha256 $patch
    if ($have -ne $patchSha) {
        Write-Host "[third-party] third-party\patches\$name.patch does not match its pin in fetch.manifest"
        Write-Host "[third-party]   pin  $patchSha"
        Write-Host "[third-party]   file $have"
        Write-Host "[third-party] if the patch was changed on purpose, put the new sha256 in fetch.manifest"
        return $false
    }

    $crate = Join-Path $Downloads "$name-$version.crate"
    if (-not ((Test-Path -LiteralPath $crate) -and ((Get-Sha256 $crate) -eq $crateSha))) {
        $url = "https://static.crates.io/crates/$name/$name-$version.crate"
        if (-not (Get-Verified $url $crateSha $crate "$name $version from crates.io")) { return $false }
    }

    if (Test-Path -LiteralPath $Staging) { Remove-Item -Recurse -Force -LiteralPath $Staging }
    New-Item -ItemType Directory -Force -Path $Staging | Out-Null
    $r = Invoke-Native $Tar @('-xzf', $crate, '-C', $Staging)
    $unpacked = Join-Path $Staging "$name-$version"
    if ($r.Code -ne 0 -or -not (Test-Path -LiteralPath (Join-Path $unpacked 'Cargo.toml'))) {
        Write-Host "[third-party] could not unpack $name-$version.crate: $($r.Out)"
        return $false
    }

    # From the root, so the paths git sees are the same inside a checkout and
    # in a source tree that is not one.
    Push-Location $Root
    try {
        $r = Invoke-Native 'git' @('apply', '-p1', '--whitespace=nowarn',
            "--directory=third-party/crates/.staging/$name-$version", "third-party/patches/$name.patch")
    } finally {
        Pop-Location
    }
    if ($r.Code -ne 0) {
        Write-Host "[third-party] third-party\patches\$name.patch does not apply to $name $version"
        Write-Host $r.Out
        return $false
    }

    if (Test-Path -LiteralPath $dest) { Remove-Item -Recurse -Force -LiteralPath $dest }
    Move-Item -LiteralPath $unpacked -Destination $dest
    [System.IO.File]::WriteAllText($mark, "$Line`n")
    Write-Host "[third-party] $name $version, patched, in third-party\crates\$name"
    return $true
}

function Install-File([string]$Line, [string[]]$Fields) {
    $path, $sha, $url = $Fields[0..2]
    $flags = @($Fields | Select-Object -Skip 3)
    $dest = Join-Path $Root ($path -replace '/', '\')
    $mark = Join-Path $Marks ($path -replace '/', '__')
    if ((Test-Path -LiteralPath $dest) -and (Test-Path -LiteralPath $mark) -and
        ((Get-Content -Raw -LiteralPath $mark).Trim() -eq $Line)) {
        return $true
    }
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $dest) | Out-Null
    if (-not (Get-Verified $url $sha $dest $path)) { return $false }
    if ($flags -contains 'crlf') {
        $text = [System.IO.File]::ReadAllText($dest) -replace "`r?`n", "`r`n"
        [System.IO.File]::WriteAllText($dest, $text)
    }
    [System.IO.File]::WriteAllText($mark, "$Line`n")
    Write-Host "[third-party] placed $path"
    return $true
}

$failed = $false
foreach ($line in Get-Content -LiteralPath $Manifest) {
    $line = $line.Trim()
    if (-not $line -or $line.StartsWith('#')) { continue }
    $fields = $line -split '\s+'
    $kind = $fields[0]
    $rest = @($fields | Select-Object -Skip 1)
    if ($kind -eq 'crate') { $ok = Install-Crate $line $rest }
    elseif ($kind -eq 'file') { $ok = Install-File $line $rest }
    else { Write-Host "[third-party] fetch.manifest: unknown entry '$kind'"; $ok = $false }
    if (-not $ok) { $failed = $true }
}

if (Test-Path -LiteralPath $Staging) { Remove-Item -Recurse -Force -LiteralPath $Staging }
if ($failed) { exit 1 }
Copy-Item -Force -LiteralPath $Manifest -Destination $Stamp
exit 0
