# SPDX-License-Identifier: MPL-2.0

# Place the sherpa-onnx prebuilt native archive this tree links against, and
# verify it against the hash pinned in crates/koru-voice-sherpa/src/artifacts.rs.
#
# Why this exists: sherpa-onnx-sys' build script downloads a matching prebuilt
# archive from GitHub whenever SHERPA_ONNX_LIB_DIR is unset, so an enabled build
# reaches the network and gets whatever the release page is serving that day. It
# also reads SHERPA_ONNX_ARCHIVE_DIR first and copies from there instead of
# downloading. That is the seam this script fills - the same discipline
# assets\fonts\*.ttf follows through koru.bat fonts.
#
#   scripts\setup\fetch-voice-runtime.ps1            this host's target
#   scripts\setup\fetch-voice-runtime.ps1 -All       all three shipped targets
#   scripts\setup\fetch-voice-runtime.ps1 -Verify    check what is already here
#
# Then, for a benchmark or a release build:
#
#   $env:SHERPA_ONNX_ARCHIVE_DIR = "$PWD\third-party\sherpa-onnx\1.13.6"
#   cargo run -p koru-voice-bench --features sherpa -- ...
#
# whisper.cpp needs no artifact: whisper-rs-sys vendors the sources inside its
# own .crate and builds them with cmake and bindgen. What it needs is CMake, a
# C++ toolchain and libclang on the machine, which this script does not install.
#
# ASCII only. Windows PowerShell 5.1 reads a BOM-less script as ANSI, and a
# UTF-8 dash here arrives as three cp1252 characters, one of which is a curly
# quote that silently opens a string and breaks the parse pages later.

[CmdletBinding()]
param(
    [switch]$All,
    [switch]$Verify
)

$ErrorActionPreference = 'Stop'

$Version = '1.13.6'
$Base = "https://github.com/k2-fsa/sherpa-onnx/releases/download/v$Version"
$Root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$Dest = Join-Path $Root "third-party\sherpa-onnx\$Version"

# The same three columns as koru_voice_sherpa::artifacts::NAMES, and a test
# there fails if the file names drift from what sherpa-onnx-sys looks for.
#
# All three are upstream's TTS-free ("no-tts") archives: no espeak-ng, ucd or
# piper_phonemize, so no GPL-3.0 eSpeak NG in what links them
# (third-party/patches/sherpa-onnx.md).
$Artifacts = @(
    [pscustomobject]@{
        Name = "sherpa-onnx-v$Version-win-x64-static-MT-Release-no-tts-lib.tar.bz2"
        Sha  = '6c746e0fabc52df40e185662faccb8dc03bcb1fd137394c3e9d3815e78ca9308'
        Size = 116816761
        Host = 'windows-x86_64'
    },
    [pscustomobject]@{
        Name = "sherpa-onnx-v$Version-osx-arm64-static-no-tts-lib.tar.bz2"
        Sha  = '5df455ff1c262347dde37e5608d2a80ff948f9d65984709cf351c4d06edc931b'
        Size = 18717755
        Host = 'macos-aarch64'
    },
    [pscustomobject]@{
        Name = "sherpa-onnx-v$Version-osx-x64-static-no-tts-lib.tar.bz2"
        Sha  = 'adcc1de32d99410416a98d4f880ee2e2ef4343fad8b627e58d1cd1a5bd6d16b8'
        Size = 18413779
        Host = 'macos-x86_64'
    }
)

# **Worktrees copy before they download.** A new worktree takes an archive from
# the main checkout or another worktree that has it, as fonts do, rather than
# downloading it again. Only a copy that matches the pin is taken, so a sibling
# holding a stale or locally built archive is passed over, not copied in to be
# refused.
function Find-SiblingArtifact {
    param([object]$Artifact)

    $ours = [System.IO.Path]::GetFullPath($Root)
    $porcelain = $null
    try { $porcelain = & git -C $Root worktree list --porcelain 2>$null } catch { }
    foreach ($line in @($porcelain)) {
        if ($line -notmatch '^worktree (.+)$') { continue }
        $tree = [System.IO.Path]::GetFullPath($Matches[1])
        if ($tree -eq $ours) { continue }
        $path = Join-Path $tree "third-party\sherpa-onnx\$Version\$($Artifact.Name)"
        if (-not (Test-Path -LiteralPath $path)) { continue }
        if ((Get-Item -LiteralPath $path).Length -ne $Artifact.Size) { continue }
        $have = (Get-FileHash -Algorithm SHA256 -LiteralPath $path).Hash.ToLowerInvariant()
        if ($have -eq $Artifact.Sha) { return $path }
    }
    return $null
}

function Place-Artifact {
    param([object]$Artifact)

    $final = Join-Path $Dest $Artifact.Name

    if ((-not (Test-Path $final)) -and (-not $Verify)) {
        $sibling = Find-SiblingArtifact -Artifact $Artifact
        if ($sibling) {
            if (-not (Test-Path $Dest)) {
                New-Item -ItemType Directory -Force -Path $Dest | Out-Null
            }
            Write-Host "[voice] copying $($Artifact.Name) from $(Split-Path -Parent $sibling)"
            $part = "$final.part"
            Copy-Item -LiteralPath $sibling -Destination $part -Force
            Move-Item -Force $part $final
        }
    }

    if (Test-Path $final) {
        $have = (Get-FileHash -Algorithm SHA256 -Path $final).Hash.ToLowerInvariant()
        if ($have -eq $Artifact.Sha) {
            Write-Host "[voice] $($Artifact.Name) is already here and verified"
            return $true
        }
        Write-Host "[voice] $($Artifact.Name) is here and does NOT match the pinned hash"
        Write-Host "[voice]   want $($Artifact.Sha)"
        Write-Host "[voice]   have $have"
        Write-Host "[voice] refusing to use it; delete it to re-fetch"
        return $false
    }

    if ($Verify) {
        Write-Host "[voice] $($Artifact.Name) is not here"
        return $false
    }

    if (-not (Test-Path $Dest)) {
        New-Item -ItemType Directory -Force -Path $Dest | Out-Null
    }

    # Verified into place, never in place: a download that is interrupted must
    # not leave a file the build script will happily copy.
    $part = "$final.part"
    if (Test-Path $part) { Remove-Item -Force $part }
    Write-Host "[voice] downloading $($Artifact.Name)"
    # Invoke-WebRequest rather than curl.exe: under $ErrorActionPreference =
    # 'Stop' a native command writing to stderr is a terminating error, and a
    # progress line on stderr would kill the script mid-download.
    $ProgressPreference = 'SilentlyContinue'
    Invoke-WebRequest -Uri "$Base/$($Artifact.Name)" -OutFile $part -UseBasicParsing

    $size = (Get-Item $part).Length
    if ($size -ne $Artifact.Size) {
        Remove-Item -Force $part
        Write-Host "[voice] $($Artifact.Name) is $size bytes and the pin says $($Artifact.Size)"
        return $false
    }
    $have = (Get-FileHash -Algorithm SHA256 -Path $part).Hash.ToLowerInvariant()
    if ($have -ne $Artifact.Sha) {
        Remove-Item -Force $part
        Write-Host "[voice] $($Artifact.Name) does not match the pinned sha256"
        Write-Host "[voice]   want $($Artifact.Sha)"
        Write-Host "[voice]   have $have"
        return $false
    }
    Move-Item -Force $part $final
    Write-Host "[voice] placed $final"
    return $true
}

$wanted = 'windows-x86_64'
$ok = $true
foreach ($artifact in $Artifacts) {
    if ((-not $All) -and ($artifact.Host -ne $wanted)) { continue }
    if (-not (Place-Artifact -Artifact $artifact)) { $ok = $false }
}

if ($ok) {
    Write-Host ''
    Write-Host "[voice] now:  `$env:SHERPA_ONNX_ARCHIVE_DIR = `"$Dest`""
    exit 0
}
exit 1
