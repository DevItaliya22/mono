# SPDX-License-Identifier: MPL-2.0

# Download the seven Hunspell dictionaries spell check develops against, each
# with its upstream README and license files.
#
# They are deliberately not committed and deliberately not yet bundled: the
# spell-check plan (D2) downloads for development and testing first, and the
# bundle-vs-language-packs decision is taken at the end with real sizes and
# corpus results in hand. Until then this script is how a machine gets
# dictionaries, and `KORU_DICTIONARIES` is how tests find them:
#
#   scripts/setup/fetch-dictionaries.ps1 -Dest D:\koru-dictionaries
#   $env:KORU_DICTIONARIES = "D:\koru-dictionaries"
#   cargo test -p koru-spell
#
# Layout is the one `koru_spell::DictionaryPaths` reads: one directory per
# BCP-47 tag holding `dictionary.aff` + `dictionary.dic` and the language's
# notices - its upstream README and license files, verbatim, under their
# upstream names - plus a `manifest.toml` at the root recording, per language,
# the upstream paths, the revision, the license as declared, the license Koru
# takes where upstream offers a choice, and whether it is GPL whatever is
# chosen. **The manifest is a record, not a review**: every entry ships only
# after the P8 license gate; `redistribution` stays "unreviewed" until then.
#
# **A dictionary never travels without its license** (licensing/THIRD-PARTY.md,
# item 2): separate files, one directory per language, never compiled into a
# binary, never merged with Koru's own word lists.
#
# Sources are the LibreOffice dictionaries tree - the same files every
# LibreOffice install spell checks with - pinned to one ref so two machines
# fetch identical bytes. The table below is `koru_spell::sources` row for row;
# a test fails if either drifts, so change both together.
#
# Keep this in step with `scripts/setup/fetch-dictionaries.sh`: the two write
# byte-identical trees, manifest included.

param(
    [string]$Dest = $env:KORU_DICTIONARIES
)

$ErrorActionPreference = "Stop"
# Windows PowerShell's progress bar slows Invoke-WebRequest by an order of
# magnitude on multi-megabyte files.
$ProgressPreference = "SilentlyContinue"

if (-not $Dest) {
    Write-Error "No destination: pass -Dest or set KORU_DICTIONARIES. This script does not guess a directory."
}

# One ref for every file, so a re-run is reproducible and the manifest's
# `revision` means something. Bump deliberately, and re-run the corpus after.
$Ref = "24.8.2.1"
$Base = "https://raw.githubusercontent.com/LibreOffice/dictionaries/libreoffice-$Ref"
$Tree = "https://github.com/LibreOffice/dictionaries/tree/libreoffice-$Ref"

# tag|aff|dic|notices (space-separated)|license declared|license Koru takes|GPL only|note
$Languages = @(
    'en-US|en/en_US.aff|en/en_US.dic|en/README_en_US.txt|LicenseRef-SCOWL|LicenseRef-SCOWL|no|BSD-style notices (Atkinson, Titze, Kuenning, UKACD) in README_en_US.txt, which UKACD requires verbatim'
    'en-GB|en/en_GB.aff|en/en_GB.dic|en/README_en_GB.txt es/LGPLv3.txt es/GPLv3.txt|LGPL-3.0-only|LGPL-3.0-only|no|README_en_GB.txt: LGPL (v3 per its changelog), credits (c) OpenStreetMap contributors (ODbL), which must be kept; license texts from es/'
    'es|es/es_ES.aff|es/es_ES.dic|es/README_hunspell_es.txt es/LICENSE.md es/MPL-1.1.txt|GPL-3.0-or-later OR LGPL-3.0-or-later OR MPL-1.1+|MPL-1.1+|no|Koru takes MPL-1.1 or later (LICENSE.md, MPL-1.1.txt)'
    'fr|fr_FR/fr.aff|fr_FR/fr.dic|fr_FR/README_fr.txt|MPL-2.0|MPL-2.0|no|Same license as Koru; the upstream directory carries no license text of its own'
    'pt-BR|pt_BR/pt_BR.aff|pt_BR/pt_BR.dic|pt_BR/README_pt_BR.txt pt_BR/README_en.txt es/MPL-1.1.txt|LGPL-3.0-only OR MPL-1.1+|MPL-1.1+|no|The README names the Mozilla Public License without a version; MPL-1.1 or later is our reading; text from es/'
    'de|de/de_DE_frami.aff|de/de_DE_frami.dic|de/README_de_DE_frami.txt de/COPYING_GPLv2 de/COPYING_GPLv3|GPL-2.0-only OR GPL-3.0-only|GPL-2.0-only OR GPL-3.0-only|yes|GPL only: ship as separate unmodified data beside the app, or replace; the OASIS-0.1 alternative covers only ODF-primary programs, which Koru is not'
    'ar|ar/ar.aff|ar/ar.dic|ar/README_ar.txt ar/COPYING.txt ar/AUTHORS.txt es/MPL-1.1.txt|GPL-2.0-or-later OR LGPL-2.1-or-later OR MPL-1.1+|MPL-1.1+|no|Koru takes MPL-1.1 or later (COPYING.txt); text from es/'
)

function Fetch([string]$Url, [string]$Target) {
    Write-Host "fetch $Url"
    Invoke-WebRequest -Uri $Url -OutFile $Target -UseBasicParsing
}

New-Item -ItemType Directory -Force $Dest | Out-Null
# .NET resolves relative paths against the process directory, not this
# session's location; resolve once so every write lands in the same tree.
$Dest = (Resolve-Path $Dest).Path
$manifest = @(
    '# Written by scripts/setup/fetch-dictionaries (.ps1 or .sh; both write these exact bytes).',
    '# A record, not a license review: redistribution stays "unreviewed" until plan D2/P8.',
    '',
    'format = 2',
    ''
)

foreach ($row in $Languages) {
    $tag, $aff, $dic, $notices, $declared, $takes, $gpl, $note = $row -split '\|'
    $dir = Join-Path $Dest $tag
    New-Item -ItemType Directory -Force $dir | Out-Null
    Fetch "$Base/$aff" (Join-Path $dir "dictionary.aff")
    Fetch "$Base/$dic" (Join-Path $dir "dictionary.dic")
    $list = @()
    foreach ($notice in ($notices -split ' ')) {
        Fetch "$Base/$notice" (Join-Path $dir $notice.Substring($notice.LastIndexOf('/') + 1))
        $list += "`"$notice`""
    }
    $chosen = if ($declared -ceq $takes) { "false" } else { "true" }
    $gplOnly = if ($gpl -eq "yes") { "true" } else { "false" }
    $manifest += @(
        "[[dictionary]]",
        "language = `"$tag`"",
        "source = `"$Tree/$($aff.Substring(0, $aff.LastIndexOf('/')))`"",
        "revision = `"libreoffice-$Ref`"",
        "aff = `"$aff`"",
        "dic = `"$dic`"",
        "notices = [$($list -join ', ')]",
        "license_declared = `"$declared`"",
        "license = `"$takes`"",
        "license_chosen = $chosen",
        "gpl_only = $gplOnly",
        "note = `"$note`"",
        "redistribution = `"unreviewed`"",
        ""
    )
}

# UTF-8 without a BOM and with LF endings, so this manifest and the .sh one
# are the same bytes (Windows PowerShell's `-Encoding utf8` would add a BOM).
$text = ($manifest -join "`n") + "`n"
[System.IO.File]::WriteAllText((Join-Path $Dest "manifest.toml"), $text, (New-Object System.Text.UTF8Encoding $false))
Write-Host "done: $($Languages.Count) languages under $Dest"
