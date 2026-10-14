# SPDX-License-Identifier: MPL-2.0

# Download the fonts the editor bundles at runtime.
#
# They are deliberately not committed: NotoSansKR alone is ~10 MB and would sit
# in git history forever. The app degrades gracefully without them -- each family
# falls back to the system sans, and Hangul to whatever the OS ships -- but the
# rendering only matches across platforms with these files present.
#
# **Most text families are fetched as a roman *and* an italic file.** They are
# not optional extras: a family without its italic file means Ctrl+I renders a
# mechanically slanted roman instead of real italic letterforms. Two are
# exceptions, called out below: Manrope ships no italic (Ctrl+I faux-skews, by
# design), and Poppins ships only static instances per weight.
#
# They land in assets/fonts/ (the Android app's three in assets/fonts/android/),
# keyed by their path under it in scripts/setup/fonts.manifest. A tree still on
# the old flat layout (assets/<name>, assets/mobile/fonts/<name>) has its files
# moved into place first, so nothing is downloaded again.
#
# **Worktrees copy before they download.** A linked worktree looks for the same
# filenames under the main checkout and sibling worktrees (and optional
# KORU_FONTS_DIR) so a second tree does not re-pull ~22 MB from the network.
# A sibling still on the old layout counts too.
#
# All text families are SIL Open Font License 1.1; Phosphor is MIT.
#
# This is what `koru.bat fonts` runs, so it is the one that matters on Windows.
# Keep it in step with `scripts/setup/fetch-fonts.sh` -- the two drifting is how this
# script ended up without the icon font for a while.
$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$assets = Join-Path $root 'assets'
$fontsDir = Join-Path $assets 'fonts'
New-Item -ItemType Directory -Force -Path $fontsDir | Out-Null
$manifestPath = Join-Path $PSScriptRoot 'fonts.manifest'
$manifest = @{}
foreach ($line in Get-Content -LiteralPath $manifestPath) {
    if (-not $line -or $line.StartsWith('#')) { continue }
    $key, $value = $line.Split('=', 2)
    $manifest[$key] = $value
}

function Test-ManifestFont {
    param([string]$Name, [string]$Path)
    if (-not $manifest.ContainsKey($Name)) { return $true }
    $actual = (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
    return $actual -eq $manifest[$Name]
}

# Ordered candidate directories that may already hold the bundled faces.
# Never includes this tree's own assets/.
function Get-FontSourceDirs {
    param([string]$Ours)

    $oursFull = [System.IO.Path]::GetFullPath($Ours)
    $seen = New-Object 'System.Collections.Generic.HashSet[string]' `
        ([StringComparer]::OrdinalIgnoreCase)
    $ordered = New-Object System.Collections.Generic.List[string]

    function Add-Candidate([string]$dir) {
        if (-not $dir) { return }
        if (-not (Test-Path -LiteralPath $dir -PathType Container)) { return }
        $full = [System.IO.Path]::GetFullPath($dir)
        if ($full -eq $oursFull) { return }
        if (-not $seen.Add($full)) { return }
        $ordered.Add($full) | Out-Null
    }

    if ($env:KORU_FONTS_DIR) {
        Add-Candidate $env:KORU_FONTS_DIR
    }

    # Main checkout: git-common-dir is .../repo/.git for both the main tree and
    # linked worktrees; its parent is the main working tree root.
    $gitCommon = $null
    try {
        $gitCommon = (& git -C $root rev-parse --git-common-dir 2>$null | Out-String).Trim()
    } catch { }
    if ($gitCommon) {
        if (-not [System.IO.Path]::IsPathRooted($gitCommon)) {
            $gitCommon = Join-Path $root $gitCommon
        }
        $gitCommon = [System.IO.Path]::GetFullPath($gitCommon)
        $leaf = Split-Path -Leaf $gitCommon
        if ($leaf -eq '.git') {
            Add-Candidate (Join-Path (Split-Path -Parent $gitCommon) 'assets')
        }
    }

    # Sibling worktrees (and the main tree again if porcelain lists it).
    $porcelain = $null
    try {
        $porcelain = & git -C $root worktree list --porcelain 2>$null
    } catch { }
    if ($porcelain) {
        foreach ($line in $porcelain) {
            if ($line -match '^worktree (.+)$') {
                Add-Candidate (Join-Path $Matches[1] 'assets')
            }
        }
    }

    # Prefer directories that already look complete (sentinel Inter.ttf, on
    # either layout).
    $withSentinel = New-Object System.Collections.Generic.List[string]
    $without = New-Object System.Collections.Generic.List[string]
    foreach ($dir in $ordered) {
        if ((Test-NonEmpty (Join-Path $dir 'fonts\Inter.ttf')) -or (Test-NonEmpty (Join-Path $dir 'Inter.ttf'))) {
            $withSentinel.Add($dir) | Out-Null
        } else {
            $without.Add($dir) | Out-Null
        }
    }
    return @($withSentinel) + @($without)
}

function Test-NonEmpty([string]$Path) {
    return (Test-Path -LiteralPath $Path -PathType Leaf) -and ((Get-Item -LiteralPath $Path).Length -gt 0)
}

# Where a font keyed `<key>` (its path under assets/fonts/) sat before fonts
# moved there: assets/<name>, and assets/mobile/fonts/<name> for android/<name>.
function Get-LegacyName([string]$Key) {
    if ($Key.StartsWith('android/')) { return 'mobile/fonts/' + $Key.Substring(8) }
    return $Key
}

# A candidate is an assets/ directory on the current layout (fonts/<key>) or the
# old one, or a KORU_FONTS_DIR that holds the faces directly (<key>).
function Find-LocalFont {
    param(
        [string]$Name,
        [string[]]$Sources
    )
    foreach ($dir in $Sources) {
        foreach ($rel in @("fonts/$Name", $Name, (Get-LegacyName $Name))) {
            $path = Join-Path $dir $rel
            if (Test-NonEmpty $path) {
                return $path
            }
        }
    }
    return $null
}

# This tree on the old layout: move its faces into assets/fonts/ rather than
# fetch them again. One that already exists at the new path is left alone.
$moved = 0
foreach ($pair in @(@($assets, $fontsDir), @((Join-Path $assets 'mobile\fonts'), (Join-Path $fontsDir 'android')))) {
    $from, $to = $pair
    if (-not (Test-Path -LiteralPath $from -PathType Container)) { continue }
    foreach ($file in @(Get-ChildItem -LiteralPath $from -Filter '*.ttf' -File)) {
        $target = Join-Path $to $file.Name
        if (Test-Path -LiteralPath $target) { continue }
        New-Item -ItemType Directory -Force -Path $to | Out-Null
        Move-Item -LiteralPath $file.FullName -Destination $target
        $moved++
    }
}
foreach ($dir in @((Join-Path $assets 'mobile\fonts'), (Join-Path $assets 'mobile'))) {
    if ((Test-Path -LiteralPath $dir -PathType Container) -and -not (Get-ChildItem -LiteralPath $dir -Force)) {
        Remove-Item -LiteralPath $dir
    }
}
if ($moved -gt 0) {
    Write-Host "moved   $moved fonts from the old layout into assets/fonts/"
}

$sources = @(Get-FontSourceDirs -Ours $assets)
if ($sources.Count -gt 0) {
    Write-Host ("font sources: " + ($sources -join '; '))
}

$gf = "https://raw.githubusercontent.com/google/fonts/$($manifest['google-fonts-revision'])/ofl"

$fonts = [ordered]@{
    # The four original UI families. Each is a variable font with a `wght`
    # axis, which is what lets the app ask for a real weight of 500 rather than
    # the faux-bold stroke a static face would get.
    'Inter.ttf'                  = "$gf/inter/Inter%5Bopsz,wght%5D.ttf"
    'Inter-Italic.ttf'           = "$gf/inter/Inter-Italic%5Bopsz,wght%5D.ttf"
    'SourceSans3.ttf'            = "$gf/sourcesans3/SourceSans3%5Bwght%5D.ttf"
    'SourceSans3-Italic.ttf'     = "$gf/sourcesans3/SourceSans3-Italic%5Bwght%5D.ttf"
    'PlusJakartaSans.ttf'        = "$gf/plusjakartasans/PlusJakartaSans%5Bwght%5D.ttf"
    'PlusJakartaSans-Italic.ttf' = "$gf/plusjakartasans/PlusJakartaSans-Italic%5Bwght%5D.ttf"
    'IBMPlexSans.ttf'            = "$gf/ibmplexsans/IBMPlexSans%5Bwdth,wght%5D.ttf"
    'IBMPlexSans-Italic.ttf'     = "$gf/ibmplexsans/IBMPlexSans-Italic%5Bwdth,wght%5D.ttf"

    # New sans. Poppins has no variable font -- it ships a static instance per
    # weight, so the italic members are fetched as separate files (eight in all)
    # to cover the app's weight ramp with real italic. Manrope ships a variable
    # roman but **no italic face** -- Ctrl+I faux-skews for Manrope, by design.
    'Poppins-Regular.ttf'        = "$gf/poppins/Poppins-Regular.ttf"
    'Poppins-Italic.ttf'         = "$gf/poppins/Poppins-Italic.ttf"
    'Poppins-Medium.ttf'         = "$gf/poppins/Poppins-Medium.ttf"
    'Poppins-MediumItalic.ttf'   = "$gf/poppins/Poppins-MediumItalic.ttf"
    'Poppins-SemiBold.ttf'       = "$gf/poppins/Poppins-SemiBold.ttf"
    'Poppins-SemiBoldItalic.ttf' = "$gf/poppins/Poppins-SemiBoldItalic.ttf"
    'Poppins-Bold.ttf'           = "$gf/poppins/Poppins-Bold.ttf"
    'Poppins-BoldItalic.ttf'     = "$gf/poppins/Poppins-BoldItalic.ttf"
    'Manrope.ttf'                = "$gf/manrope/Manrope%5Bwght%5D.ttf"

    # Serif.
    'SourceSerif4.ttf'           = "$gf/sourceserif4/SourceSerif4%5Bopsz,wght%5D.ttf"
    'SourceSerif4-Italic.ttf'    = "$gf/sourceserif4/SourceSerif4-Italic%5Bopsz,wght%5D.ttf"
    'Lora.ttf'                   = "$gf/lora/Lora%5Bwght%5D.ttf"
    'Lora-Italic.ttf'            = "$gf/lora/Lora-Italic%5Bwght%5D.ttf"

    # Rounded.
    'Nunito.ttf'                 = "$gf/nunito/Nunito%5Bwght%5D.ttf"
    'Nunito-Italic.ttf'          = "$gf/nunito/Nunito-Italic%5Bwght%5D.ttf"

    # Mono (bundled, so code renders identically across platforms). The system
    # `Mono` stack remains a selectable fallback that fetches nothing.
    'JetBrainsMono.ttf'          = "$gf/jetbrainsmono/JetBrainsMono%5Bwght%5D.ttf"
    'JetBrainsMono-Italic.ttf'   = "$gf/jetbrainsmono/JetBrainsMono-Italic%5Bwght%5D.ttf"
    'SourceCodePro.ttf'          = "$gf/sourcecodepro/SourceCodePro%5Bwght%5D.ttf"
    'SourceCodePro-Italic.ttf'   = "$gf/sourcecodepro/SourceCodePro-Italic%5Bwght%5D.ttf"

    # Hangul, for every family -- it trails each stack rather than being a choice.
    'NotoSansKR-Regular.ttf'     = 'https://github.com/notofonts/noto-cjk/raw/main/Sans/Variable/TTF/Subset/NotoSansKR-VF.ttf'

    # Pinned: crates/koru-draw/src/icons.rs holds codepoints lifted from this
    # release's stylesheet, and a different release could move them.
    'Phosphor.ttf'               = 'https://cdn.jsdelivr.net/npm/@phosphor-icons/web@2.1.2/src/regular/Phosphor.ttf'

    # The Fill weight, same release and the same codepoints - a separate family
    # rather than a weight, because that is how Phosphor ships it.
    'Phosphor-Fill.ttf'          = 'https://cdn.jsdelivr.net/npm/@phosphor-icons/web@2.1.2/src/fill/Phosphor-Fill.ttf'

    # The Android app's faces, in assets/fonts/android/, the directory its
    # Gradle build reads as an asset source (Theme.kt loads them by file name).
    # Static, because that is what Theme.kt asks for: Google Fonts' own
    # instances of the families above, pinned by hash in
    # scripts/setup/fonts.manifest.
    'android/Inter-Regular.ttf'    = 'https://fonts.gstatic.com/s/inter/v20/UcCO3FwrK3iLTeHuS_nVMrMxCp50SjIw2boKoduKmMEVuLyfMZg.ttf'
    'android/Inter-Medium.ttf'     = 'https://fonts.gstatic.com/s/inter/v20/UcCO3FwrK3iLTeHuS_nVMrMxCp50SjIw2boKoduKmMEVuI6fMZg.ttf'
    'android/Manrope-SemiBold.ttf' = 'https://fonts.gstatic.com/s/manrope/v20/xn7_YHE41ni1AdIRqAuZuw1Bx9mbZk4jE-_F.ttf'
}

foreach ($name in $fonts.Keys) {
    $dest = Join-Path $fontsDir $name
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $dest) | Out-Null
    if ((Test-Path -LiteralPath $dest) -and ((Get-Item -LiteralPath $dest).Length -gt 0)) {
        if (Test-ManifestFont -Name $name -Path $dest) {
            Write-Host "have    $name"
            continue
        }
        Write-Host "refresh $name (manifest hash mismatch)"
    }
    # .part first so an interrupted run never leaves a truncated file that looks
    # valid to the app (same rule for copy and download).
    $part = "$dest.part"
    $local = Find-LocalFont -Name $name -Sources $sources
    if ($local) {
        Write-Host "copy    $name"
        Copy-Item -LiteralPath $local -Destination $part -Force
        if (-not (Test-ManifestFont -Name $name -Path $part)) {
            Remove-Item -Force -LiteralPath $part
            throw "$name copied bytes do not match scripts/setup/fonts.manifest"
        }
        Move-Item -Force -LiteralPath $part -Destination $dest
        continue
    }
    Write-Host "fetch   $name"
    Invoke-WebRequest -Uri $fonts[$name] -OutFile $part -UseBasicParsing
    if (-not (Test-ManifestFont -Name $name -Path $part)) {
        Remove-Item -Force -LiteralPath $part
        throw "$name downloaded bytes do not match scripts/setup/fonts.manifest"
    }
    Move-Item -Force -LiteralPath $part -Destination $dest
}

Get-ChildItem $fontsDir | Select-Object Name, Length
