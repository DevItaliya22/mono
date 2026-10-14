# SPDX-License-Identifier: MPL-2.0

# Exclude Koru's repository from Windows Defender real-time scanning.
#
# ASCII ONLY. Windows PowerShell 5.1 reads a BOM-less script as ANSI, so a UTF-8
# dash or curly quote arrives as three cp1252 characters, one of which opens a
# string and breaks the parse pages later, pointing at innocent lines. Same rule
# as dist.ps1 had. Do not paste prose in here from anywhere.
#
# WHY THIS EXISTS
#
# Every .rmeta, .rlib, .o, .pdb and .exe that cargo writes is scanned as it is
# written. A target directory here is ~28 GB and a build touches thousands of
# files, so on Windows this is commonly 20-40% of build time. The source tree
# pays too: every file a checkout, a branch switch, a formatter or an editor
# save rewrites is scanned again. Excluding the repository is the single largest
# build-speed change available that does not touch the code.
#
# THE TRADE-OFF, STATED PLAINLY
#
# Defender stops scanning the repository, not only compiler output. That covers
# anything a build script or a dependency writes anywhere under it - this
# workspace runs build scripts from four crates and compiles several hundred
# dependencies from crates.io - and it covers the source trees and every
# worktree as well. It is accepted here because nothing lands under this root
# except our own checkouts and what our own tools write there. The risk is
# small and it is not zero, which is why this is a script you run deliberately
# rather than something the build does for you.
#
# It is reversible: run with -Remove.
#
# USAGE
#
#   Right-click Windows PowerShell, "Run as administrator", then:
#
#     cd <your clone>
#     .\scripts\setup\defender-exclusions.ps1              # add
#     .\scripts\setup\defender-exclusions.ps1 -WhatIf      # show, change nothing
#     .\scripts\setup\defender-exclusions.ps1 -Remove      # undo
#
#   A worktree under `.wt` needs nothing: the exclusion is recursive and covers
#   everything inside the repository the moment it exists. Re-run this only
#   after creating a worktree OUTSIDE the repo, since those are enumerated from
#   `git worktree list` one at a time.

[CmdletBinding(SupportsShouldProcess = $true)]
param(
    # Undo: remove every exclusion this script adds.
    [switch]$Remove,

    # Also exclude the compiler processes themselves, not only the directories
    # they write to. Usually worth another few percent, and it is a broader
    # grant: anything those processes touch anywhere goes unscanned. Off by
    # default so the narrow version is the one you get without asking.
    [switch]$IncludeProcesses
)

$ErrorActionPreference = "Stop"

# Under Stop, a native command writing to stderr is a terminating error. Nothing
# here shells out, but `git` does below, so it is funnelled through one wrapper
# that is allowed to fail.
function Get-GitLines {
    param([string[]]$GitArgs)
    $old = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        $out = & git @GitArgs 2>$null
        if ($LASTEXITCODE -ne 0) { return @() }
        return @($out)
    } finally {
        $ErrorActionPreference = $old
    }
}

function Test-Admin {
    $id = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = New-Object Security.Principal.WindowsPrincipal($id)
    return $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

if (-not (Test-Admin)) {
    Write-Host "This needs an elevated PowerShell." -ForegroundColor Yellow
    Write-Host ""
    Write-Host "  Right-click Windows PowerShell, 'Run as administrator', then:"
    Write-Host ""
    Write-Host "    cd $PSScriptRoot\..\.."
    Write-Host "    .\scripts\setup\defender-exclusions.ps1"
    Write-Host ""
    Write-Host "Reading the current exclusion list also requires it, so this"
    Write-Host "cannot usefully report what is already set from here."
    exit 1
}

$repo = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)

# The repository.
#
# A Defender path exclusion is recursive, so one entry for the repo root covers
# target, `.wt` and every worktree under it, and the source trees themselves.
# That is the broadest grant on offer and it is taken deliberately: the payoff
# is that anything created inside the repo is covered the moment it exists,
# rather than when someone remembers to re-run this and add a directory here.
#
# The enumeration below still earns its place for worktrees kept OUTSIDE the
# repo (a `.codex\worktrees\...` one, for instance); those are not under the
# repo root and are not covered by it.
$paths = New-Object System.Collections.Generic.List[string]

$paths.Add($repo)

$worktrees = Get-GitLines @("-C", $repo, "worktree", "list", "--porcelain") |
    Where-Object { $_ -like "worktree *" } |
    ForEach-Object { $_.Substring(9) }
$repoRoot = $repo.TrimEnd("\") + "\"
foreach ($w in $worktrees) {
    $dir = ($w -replace "/", "\").TrimEnd("\")
    # Already covered, recursively, by the repo entry above - including the main
    # worktree, which is the repo root itself.
    if (($dir + "\").StartsWith($repoRoot, [System.StringComparison]::OrdinalIgnoreCase)) { continue }
    if (-not $paths.Contains($dir)) { $paths.Add($dir) }
}

# The registry and git checkouts cargo compiles from, but NOT ~/.cargo/bin:
# those are executables you install and run, and they are exactly the kind of
# thing worth keeping scanned.
$cargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $env:USERPROFILE ".cargo" }
$paths.Add((Join-Path $cargoHome "registry"))
$paths.Add((Join-Path $cargoHome "git"))

# The shared compiler cache. Every hit reads from here and every miss writes.
#
# Some machines keep it at D:\sccache. SCCACHE_DIR is honoured when it is
# set, but it is read from whichever account the elevated shell is running as,
# which is not always the account that set it - so D:\sccache goes in whenever
# it exists (and always on -Remove) rather than being trusted to show up in the
# environment.
$sccacheDirs = New-Object System.Collections.Generic.List[string]
if ($Remove -or (Test-Path "D:\sccache")) { $sccacheDirs.Add("D:\sccache") }
if ($env:SCCACHE_DIR) { $sccacheDirs.Add($env:SCCACHE_DIR) }

# sccache's own default, from before the move. Added while it is still on disk,
# so a machine that has not moved yet is covered, and always on -Remove, so an
# undo clears it either way.
$sccacheDefault = Join-Path $env:LOCALAPPDATA "Mozilla\sccache"
if ($Remove -or (Test-Path $sccacheDefault)) { $sccacheDirs.Add($sccacheDefault) }

foreach ($d in $sccacheDirs) {
    $cache = $d.TrimEnd("\")
    if (-not $paths.Contains($cache)) { $paths.Add($cache) }
}

$processes = @("rustc.exe", "cargo.exe", "sccache.exe", "rust-lld.exe", "link.exe")

# Entries earlier versions of this script added, back when it excluded the two
# build directories rather than the repo root. An undo should leave a clean
# list, so -Remove clears them too; removing a path that is not excluded is a
# no-op, so this costs nothing on a machine that never had them.
if ($Remove) {
    $paths.Add((Join-Path $repo ".wt"))
    $paths.Add((Join-Path $repo "target"))
    $wtRoot = (Join-Path $repo ".wt") + "\"
    foreach ($w in $worktrees) {
        $dir = ($w -replace "/", "\").TrimEnd("\")
        # The old script never listed these one by one either: the `.wt` entry
        # above covered them recursively.
        if ($dir.StartsWith($wtRoot, [System.StringComparison]::OrdinalIgnoreCase)) { continue }
        $t = Join-Path $dir "target"
        if (-not $paths.Contains($t)) { $paths.Add($t) }
    }
}

$verb = if ($Remove) { "Removing" } else { "Adding" }
Write-Host "$verb Defender exclusions for the Koru repository." -ForegroundColor Cyan
Write-Host ""

foreach ($p in $paths) {
    if ($PSCmdlet.ShouldProcess($p, "$verb path exclusion")) {
        try {
            if ($Remove) {
                Remove-MpPreference -ExclusionPath $p
            } else {
                Add-MpPreference -ExclusionPath $p
            }
            Write-Host "  ok   $p"
        } catch {
            Write-Host "  FAIL $p" -ForegroundColor Red
            Write-Host "       $($_.Exception.Message)" -ForegroundColor Red
        }
    }
}

if ($IncludeProcesses) {
    Write-Host ""
    foreach ($proc in $processes) {
        if ($PSCmdlet.ShouldProcess($proc, "$verb process exclusion")) {
            try {
                if ($Remove) {
                    Remove-MpPreference -ExclusionProcess $proc
                } else {
                    Add-MpPreference -ExclusionProcess $proc
                }
                Write-Host "  ok   process $proc"
            } catch {
                Write-Host "  FAIL process $proc" -ForegroundColor Red
                Write-Host "       $($_.Exception.Message)" -ForegroundColor Red
            }
        }
    }
} elseif (-not $Remove) {
    Write-Host ""
    Write-Host "Process exclusions were not added. -IncludeProcesses adds them"
    Write-Host "(rustc, cargo, sccache, the linkers) for a few percent more, at"
    Write-Host "the cost of a broader grant."
}

Write-Host ""
Write-Host "Done. Verify with:  Get-MpPreference | Select-Object -ExpandProperty ExclusionPath"
if (-not $Remove) {
    Write-Host "Undo with:          .\scripts\setup\defender-exclusions.ps1 -Remove"
    Write-Host ""
    Write-Host "Everything inside the repo is covered recursively. Re-run only"
    Write-Host "after creating a worktree outside it."
}
