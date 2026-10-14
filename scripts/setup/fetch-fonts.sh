#!/usr/bin/env bash
# SPDX-License-Identifier: MPL-2.0

# Download the fonts the editor bundles at runtime.
#
# They are deliberately not committed: NotoSansKR alone is ~10 MB and would sit
# in git history forever. The app degrades gracefully without them — each family
# falls back to the system sans, and Hangul to whatever the OS ships (Malgun
# Gothic on Windows, Apple SD Gothic Neo on macOS, Noto on most Linux) — but the
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
# The Phosphor version is pinned: `crates/koru-draw/src/icons.rs` holds
# codepoints lifted from that release's stylesheet, and a different release
# could move them. Bump both together.
#
# Keep this in step with `scripts/setup/fetch-fonts.ps1`, which is what `koru.bat
# fonts` runs — the two drifting is how Windows ended up with no icon font.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
assets="$root/assets"
fonts="$assets/fonts"
mkdir -p "$fonts"
manifest="$root/scripts/setup/fonts.manifest"

manifest_value() {
  awk -F= -v key="$1" '$1 == key { print $2; exit }' "$manifest"
}

verify_manifest_font() {
  local name="$1" path="$2" expected actual
  expected="$(manifest_value "$name")"
  [ -n "$expected" ] || return 0
  if command -v sha256sum >/dev/null 2>&1; then
    actual="$(sha256sum "$path" | awk '{print $1}')"
  else
    actual="$(shasum -a 256 "$path" | awk '{print $1}')"
  fi
  [ "$actual" = "$expected" ]
}

# Absolute, resolved path for comparison (no trailing slash).
abs_path() {
  local p="$1"
  if command -v realpath >/dev/null 2>&1; then
    realpath -m "$p" 2>/dev/null || python3 -c 'import os,sys; print(os.path.realpath(sys.argv[1]))' "$p"
  else
    python3 -c 'import os,sys; print(os.path.realpath(sys.argv[1]))' "$p"
  fi
}

ours="$(abs_path "$assets")"

# This tree on the old layout: move its faces into assets/fonts/ rather than
# fetch them again. One that already exists at the new path is left alone.
moved=0
migrate() {
  local from="$1" to="$2" file
  for file in "$from"/*.ttf; do
    [ -f "$file" ] || continue
    [ -e "$to/$(basename "$file")" ] && continue
    mkdir -p "$to"
    mv "$file" "$to/"
    moved=$((moved + 1))
  done
}
migrate "$assets" "$fonts"
migrate "$assets/mobile/fonts" "$fonts/android"
rmdir "$assets/mobile/fonts" "$assets/mobile" 2>/dev/null || true
if [ "$moved" -gt 0 ]; then
  echo "moved   $moved fonts from the old layout into assets/fonts/"
fi

# Populate FONT_SOURCES with candidate assets dirs (excluding ours).
# Prefer dirs that already have a non-empty Inter.ttf sentinel, on either layout.
declare -a FONT_SOURCES=()
declare -a _with_sentinel=()
declare -a _without=()
# Dedup as a newline-delimited list, not `declare -A`: stock macOS ships
# bash 3.2, where associative arrays do not exist and the declare aborts the
# whole fetch under `set -e` — which is how a Mac never got its fonts.
_seen=$'\n'

add_candidate() {
  local dir="${1:-}"
  [ -n "$dir" ] || return 0
  [ -d "$dir" ] || return 0
  local full
  full="$(abs_path "$dir")"
  [ "$full" = "$ours" ] && return 0
  # Git for Windows prints D:/x where this shell says /d/x; -ef sees through it.
  [ "$full" -ef "$assets" ] && return 0
  case "$_seen" in *$'\n'"$full"$'\n'*) return 0 ;; esac
  _seen="${_seen}${full}"$'\n'
  if [ -s "$full/fonts/Inter.ttf" ] || [ -s "$full/Inter.ttf" ]; then
    _with_sentinel+=("$full")
  else
    _without+=("$full")
  fi
}

if [ -n "${KORU_FONTS_DIR:-}" ]; then
  add_candidate "$KORU_FONTS_DIR"
fi

# Main checkout: git-common-dir is .../repo/.git for main and linked worktrees.
if git_common="$(git -C "$root" rev-parse --git-common-dir 2>/dev/null)"; then
  case "$git_common" in
    /*) ;;
    *) git_common="$root/$git_common" ;;
  esac
  git_common="$(abs_path "$git_common")"
  case "$(basename "$git_common")" in
    .git) add_candidate "$(dirname "$git_common")/assets" ;;
  esac
fi

# Sibling worktrees.
if porcelain="$(git -C "$root" worktree list --porcelain 2>/dev/null)"; then
  while IFS= read -r line; do
    case "$line" in
      worktree\ *) add_candidate "${line#worktree }/assets" ;;
    esac
  done <<<"$porcelain"
fi

# The ${arr[@]+...} guards are for bash 3.2 (stock macOS), where expanding an
# empty array under `set -u` is an error rather than nothing.
FONT_SOURCES=(${_with_sentinel[@]+"${_with_sentinel[@]}"} ${_without[@]+"${_without[@]}"})

if [ "${#FONT_SOURCES[@]}" -gt 0 ]; then
  echo "font sources: ${FONT_SOURCES[*]}"
fi

# A candidate is an assets/ dir on the current layout (fonts/<key>) or the old
# one (<name>, and mobile/fonts/<name> for android/<name>), or a KORU_FONTS_DIR
# that holds the faces directly (<key>).
find_local() {
  local name="$1" dir rel legacy="$1"
  case "$name" in android/*) legacy="mobile/fonts/${name#android/}" ;; esac
  for dir in ${FONT_SOURCES[@]+"${FONT_SOURCES[@]}"}; do
    for rel in "fonts/$name" "$name" "$legacy"; do
      if [ -s "$dir/$rel" ]; then
        printf '%s\n' "$dir/$rel"
        return 0
      fi
    done
  done
  return 1
}

fetch() {
  local name="$1" url="$2"
  if [ -s "$fonts/$name" ]; then
    if verify_manifest_font "$name" "$fonts/$name"; then
      echo "have    $name"
      return
    fi
    echo "refresh $name (manifest hash mismatch)"
  fi
  # .part first so an interrupted run never leaves a truncated file that looks
  # valid to the app (same rule for copy and download).
  mkdir -p "$(dirname "$fonts/$name")"
  local part="$fonts/$name.part"
  local src
  if src="$(find_local "$name")"; then
    echo "copy    $name"
    cp "$src" "$part"
    if ! verify_manifest_font "$name" "$part"; then
      rm -f "$part"
      echo "refuse  $name (copied bytes do not match scripts/setup/fonts.manifest)" >&2
      return 1
    fi
    mv "$part" "$fonts/$name"
    return
  fi
  echo "fetch   $name"
  # -f so an HTML error page is never written out as a .ttf
  curl -fLsS "$url" -o "$part"
  if ! verify_manifest_font "$name" "$part"; then
    rm -f "$part"
    echo "refuse  $name (downloaded bytes do not match scripts/setup/fonts.manifest)" >&2
    return 1
  fi
  mv "$part" "$fonts/$name"
}

google_fonts_revision="$(manifest_value google-fonts-revision)"
gf="https://raw.githubusercontent.com/google/fonts/$google_fonts_revision/ofl"

# The four selectable UI families. Each is a variable font with a `wght` axis,
# which is what lets the app ask for a real weight of 500 rather than the
# faux-bold stroke a static face would get.
fetch Inter.ttf                    "$gf/inter/Inter%5Bopsz,wght%5D.ttf"
fetch Inter-Italic.ttf             "$gf/inter/Inter-Italic%5Bopsz,wght%5D.ttf"
fetch SourceSans3.ttf              "$gf/sourcesans3/SourceSans3%5Bwght%5D.ttf"
fetch SourceSans3-Italic.ttf       "$gf/sourcesans3/SourceSans3-Italic%5Bwght%5D.ttf"
fetch PlusJakartaSans.ttf          "$gf/plusjakartasans/PlusJakartaSans%5Bwght%5D.ttf"
fetch PlusJakartaSans-Italic.ttf   "$gf/plusjakartasans/PlusJakartaSans-Italic%5Bwght%5D.ttf"
fetch IBMPlexSans.ttf              "$gf/ibmplexsans/IBMPlexSans%5Bwdth,wght%5D.ttf"
fetch IBMPlexSans-Italic.ttf       "$gf/ibmplexsans/IBMPlexSans-Italic%5Bwdth,wght%5D.ttf"

# New sans. Poppins has no variable font — it ships a static instance per
# weight, so the italic members are separate files (eight in all) to cover the
# app's weight ramp with real italic. Manrope ships a variable roman but **no
# italic face** — Ctrl+I faux-skews for Manrope, by design.
fetch Poppins-Regular.ttf        "$gf/poppins/Poppins-Regular.ttf"
fetch Poppins-Italic.ttf         "$gf/poppins/Poppins-Italic.ttf"
fetch Poppins-Medium.ttf         "$gf/poppins/Poppins-Medium.ttf"
fetch Poppins-MediumItalic.ttf   "$gf/poppins/Poppins-MediumItalic.ttf"
fetch Poppins-SemiBold.ttf       "$gf/poppins/Poppins-SemiBold.ttf"
fetch Poppins-SemiBoldItalic.ttf "$gf/poppins/Poppins-SemiBoldItalic.ttf"
fetch Poppins-Bold.ttf           "$gf/poppins/Poppins-Bold.ttf"
fetch Poppins-BoldItalic.ttf     "$gf/poppins/Poppins-BoldItalic.ttf"
fetch Manrope.ttf                "$gf/manrope/Manrope%5Bwght%5D.ttf"

# Serif.
fetch SourceSerif4.ttf           "$gf/sourceserif4/SourceSerif4%5Bopsz,wght%5D.ttf"
fetch SourceSerif4-Italic.ttf    "$gf/sourceserif4/SourceSerif4-Italic%5Bopsz,wght%5D.ttf"
fetch Lora.ttf                   "$gf/lora/Lora%5Bwght%5D.ttf"
fetch Lora-Italic.ttf            "$gf/lora/Lora-Italic%5Bwght%5D.ttf"

# Rounded.
fetch Nunito.ttf                 "$gf/nunito/Nunito%5Bwght%5D.ttf"
fetch Nunito-Italic.ttf          "$gf/nunito/Nunito-Italic%5Bwght%5D.ttf"

# Mono (bundled, so code renders identically across platforms). The system
# `Mono` stack remains a selectable fallback that fetches nothing.
fetch JetBrainsMono.ttf          "$gf/jetbrainsmono/JetBrainsMono%5Bwght%5D.ttf"
fetch JetBrainsMono-Italic.ttf   "$gf/jetbrainsmono/JetBrainsMono-Italic%5Bwght%5D.ttf"
fetch SourceCodePro.ttf          "$gf/sourcecodepro/SourceCodePro%5Bwght%5D.ttf"
fetch SourceCodePro-Italic.ttf   "$gf/sourcecodepro/SourceCodePro-Italic%5Bwght%5D.ttf"

# Hangul, for every family — it trails each stack rather than being a choice.
fetch NotoSansKR-Regular.ttf \
  'https://github.com/notofonts/noto-cjk/raw/main/Sans/Variable/TTF/Subset/NotoSansKR-VF.ttf'

# Pinned: crates/koru-draw/src/icons.rs holds codepoints lifted from this
# release's stylesheet, and a different release could move them.
fetch Phosphor.ttf \
  'https://cdn.jsdelivr.net/npm/@phosphor-icons/web@2.1.2/src/regular/Phosphor.ttf'

# The Fill weight, same release and the *same codepoints* — a separate family
# rather than a weight, because that is how Phosphor ships it. Only a state
# that is genuinely on uses it: the pin, while the window is on top.
fetch Phosphor-Fill.ttf \
  'https://cdn.jsdelivr.net/npm/@phosphor-icons/web@2.1.2/src/fill/Phosphor-Fill.ttf'

# The Android app's faces, in assets/fonts/android/, the directory its Gradle
# build reads as an asset source (Theme.kt loads them by file name). Static,
# because that is what Theme.kt asks for: Google Fonts' own instances of the
# families above, pinned by hash in scripts/setup/fonts.manifest.
fetch android/Inter-Regular.ttf \
  'https://fonts.gstatic.com/s/inter/v20/UcCO3FwrK3iLTeHuS_nVMrMxCp50SjIw2boKoduKmMEVuLyfMZg.ttf'
fetch android/Inter-Medium.ttf \
  'https://fonts.gstatic.com/s/inter/v20/UcCO3FwrK3iLTeHuS_nVMrMxCp50SjIw2boKoduKmMEVuI6fMZg.ttf'
fetch android/Manrope-SemiBold.ttf \
  'https://fonts.gstatic.com/s/manrope/v20/xn7_YHE41ni1AdIRqAuZuw1Bx9mbZk4jE-_F.ttf'

ls -lh "$fonts"
