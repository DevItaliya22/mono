#!/usr/bin/env bash
# SPDX-License-Identifier: MPL-2.0

# Download the seven Hunspell dictionaries spell check develops against, each
# with its upstream README and license files.
#
# See scripts/setup/fetch-dictionaries.ps1 for the full story — the two must stay in
# step (the fonts pair drifting once is how Windows ended up with no icon
# font), and `koru_spell::sources` holds both to the same table row for row.
# Short form: not committed, not yet bundled (plan D2 decides delivery at P8);
# layout is one directory per BCP-47 tag with dictionary.aff + dictionary.dic
# plus the notices under their upstream names, read by
# `koru_spell::DictionaryPaths`; manifest.toml records source, revision and
# license per language and is a record, not a license review.
#
#   scripts/setup/fetch-dictionaries.sh ~/koru-dictionaries
#   KORU_DICTIONARIES=~/koru-dictionaries cargo test -p koru-spell
set -euo pipefail

dest="${1:-${KORU_DICTIONARIES:-}}"
if [ -z "$dest" ]; then
  echo "No destination: pass a directory or set KORU_DICTIONARIES." >&2
  exit 2
fi

ref="24.8.2.1"
base="https://raw.githubusercontent.com/LibreOffice/dictionaries/libreoffice-$ref"
tree="https://github.com/LibreOffice/dictionaries/tree/libreoffice-$ref"

# tag|aff|dic|notices (space-separated)|license declared|license Koru takes|GPL only|note
languages=(
  "en-US|en/en_US.aff|en/en_US.dic|en/README_en_US.txt|LicenseRef-SCOWL|LicenseRef-SCOWL|no|BSD-style notices (Atkinson, Titze, Kuenning, UKACD) in README_en_US.txt, which UKACD requires verbatim"
  "en-GB|en/en_GB.aff|en/en_GB.dic|en/README_en_GB.txt es/LGPLv3.txt es/GPLv3.txt|LGPL-3.0-only|LGPL-3.0-only|no|README_en_GB.txt: LGPL (v3 per its changelog), credits (c) OpenStreetMap contributors (ODbL), which must be kept; license texts from es/"
  "es|es/es_ES.aff|es/es_ES.dic|es/README_hunspell_es.txt es/LICENSE.md es/MPL-1.1.txt|GPL-3.0-or-later OR LGPL-3.0-or-later OR MPL-1.1+|MPL-1.1+|no|Koru takes MPL-1.1 or later (LICENSE.md, MPL-1.1.txt)"
  "fr|fr_FR/fr.aff|fr_FR/fr.dic|fr_FR/README_fr.txt|MPL-2.0|MPL-2.0|no|Same license as Koru; the upstream directory carries no license text of its own"
  "pt-BR|pt_BR/pt_BR.aff|pt_BR/pt_BR.dic|pt_BR/README_pt_BR.txt pt_BR/README_en.txt es/MPL-1.1.txt|LGPL-3.0-only OR MPL-1.1+|MPL-1.1+|no|The README names the Mozilla Public License without a version; MPL-1.1 or later is our reading; text from es/"
  "de|de/de_DE_frami.aff|de/de_DE_frami.dic|de/README_de_DE_frami.txt de/COPYING_GPLv2 de/COPYING_GPLv3|GPL-2.0-only OR GPL-3.0-only|GPL-2.0-only OR GPL-3.0-only|yes|GPL only: ship as separate unmodified data beside the app, or replace; the OASIS-0.1 alternative covers only ODF-primary programs, which Koru is not"
  "ar|ar/ar.aff|ar/ar.dic|ar/README_ar.txt ar/COPYING.txt ar/AUTHORS.txt es/MPL-1.1.txt|GPL-2.0-or-later OR LGPL-2.1-or-later OR MPL-1.1+|MPL-1.1+|no|Koru takes MPL-1.1 or later (COPYING.txt); text from es/"
)

mkdir -p "$dest"
manifest="$dest/manifest.toml"
{
  echo '# Written by scripts/setup/fetch-dictionaries (.ps1 or .sh; both write these exact bytes).'
  echo '# A record, not a license review: redistribution stays "unreviewed" until plan D2/P8.'
  echo
  echo 'format = 2'
  echo
} > "$manifest"

fetch() { # url target
  echo "fetch $1"
  curl -fsSL "$1" -o "$2"
}

for entry in "${languages[@]}"; do
  IFS='|' read -r tag aff dic notices declared takes gpl note <<< "$entry"
  mkdir -p "$dest/$tag"
  fetch "$base/$aff" "$dest/$tag/dictionary.aff"
  fetch "$base/$dic" "$dest/$tag/dictionary.dic"
  list=""
  for notice in $notices; do
    fetch "$base/$notice" "$dest/$tag/${notice##*/}"
    list="${list:+$list, }\"$notice\""
  done
  if [ "$declared" = "$takes" ]; then chosen=false; else chosen=true; fi
  if [ "$gpl" = "yes" ]; then gpl_only=true; else gpl_only=false; fi
  {
    echo "[[dictionary]]"
    echo "language = \"$tag\""
    echo "source = \"$tree/${aff%/*}\""
    echo "revision = \"libreoffice-$ref\""
    echo "aff = \"$aff\""
    echo "dic = \"$dic\""
    echo "notices = [$list]"
    echo "license_declared = \"$declared\""
    echo "license = \"$takes\""
    echo "license_chosen = $chosen"
    echo "gpl_only = $gpl_only"
    echo "note = \"$note\""
    echo "redistribution = \"unreviewed\""
    echo
  } >> "$manifest"
done

echo "done: ${#languages[@]} languages under $dest"
