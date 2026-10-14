#!/usr/bin/env bash
# SPDX-License-Identifier: MPL-2.0

# Fetch the third-party code Koru builds with but does not commit: the crates
# it builds from patched source, and a few upstream files used as they are.
#
# The workspace names the crates by path (Cargo.toml) and koru-blocks compiles
# a fetched grammar in, so every cargo command needs them on disk: koru.sh
# runs this before every verb that reaches cargo. The Android build's Gradle
# wrapper is fetched too. Nothing here is committed but the pins and the
# patches; third-party/README.md says why, and how to change one.
#
#   bash scripts/setup/fetch-third-party.sh
#   bash scripts/setup/fetch-third-party.sh --update-patch <name>
#
# For each line of third-party/fetch.manifest:
#
#   crate <name> <version> <crate sha256> <patch sha256>
#     download the .crate from crates.io, refuse it or the patch unless each
#     matches its pin, unpack, apply third-party/patches/<name>.patch with
#     `git apply`, and move the result into third-party/crates/<name>/.
#   file <path> <sha256> <url> [exec] [crlf]
#     download <url>, refuse it unless it matches, and place it at <path>
#     (relative to the root); `crlf` gives it CRLF line endings afterwards
#     (a batch file), `exec` marks it executable.
#
# An entry whose stamp already matches its line is left alone, so a second run
# touches nothing and local edits survive until the pin changes.
#
# --update-patch rewrites third-party/patches/<name>.patch from your edits to
# third-party/crates/<name>/ and prints the sha256 to pin in fetch.manifest.
#
# Keep in step with scripts/setup/fetch-third-party.ps1.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
manifest="$root/third-party/fetch.manifest"
crates="$root/third-party/crates"
stamp="$crates/.fetched"
downloads="$crates/.download"
marks="$crates/.files"

sha256() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  else
    shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

if [ "${1:-}" = "--update-patch" ]; then
  name="${2:?usage: fetch-third-party.sh --update-patch <name>}"
  version="$(awk -v n="$name" '$1 == "crate" && $2 == n { print $3 }' "$manifest" | tr -d '\r')"
  if [ -z "$version" ]; then
    echo "[third-party] no crate $name in third-party/fetch.manifest" >&2
    exit 1
  fi
  crate="$downloads/$name-$version.crate"
  if [ ! -d "$crates/$name" ] || [ ! -f "$crate" ]; then
    echo "[third-party] fetch first: third-party/crates/$name and its .crate are not both here" >&2
    exit 1
  fi
  # Outside the checkout, so this repository's .gitattributes cannot rewrite
  # line endings in upstream's files on their way into the patch.
  work="$(mktemp -d)"
  tar -xzf "$crate" -C "$work"
  mv "$work/$name-$version" "$work/a"
  cp -R "$crates/$name" "$work/b"
  rm -f "$work/b/.koru-fetched"
  patch="$root/third-party/patches/$name.patch"
  # `git diff --no-index` exits 1 when the trees differ, which they must.
  status=0
  (cd "$work" && git diff --no-index --no-prefix --output="$patch" a b) || status=$?
  rm -rf "$work"
  if [ "$status" -gt 1 ]; then
    echo "[third-party] git diff failed" >&2
    exit 1
  fi
  echo "[third-party] wrote third-party/patches/$name.patch"
  echo "[third-party] pin it in fetch.manifest: $(sha256 "$patch")"
  exit 0
fi

# The whole manifest, byte for byte, is the stamp of a complete fetch.
if [ -f "$stamp" ] && cmp -s "$manifest" "$stamp"; then
  exit 0
fi

staging="$crates/.staging"
mkdir -p "$downloads" "$marks"

# Verified into place, never in place: an interrupted download must not leave
# a file that a later run takes for the real one.
get_verified() { # url sha out what
  echo "[third-party] downloading $4"
  curl -fsSL -o "$3.part" "$1"
  local have
  have="$(sha256 "$3.part")"
  if [ "$have" != "$2" ]; then
    rm -f "$3.part"
    echo "[third-party] $4 does not match its pin in fetch.manifest" >&2
    echo "[third-party]   pin  $2" >&2
    echo "[third-party]   got  $have" >&2
    return 1
  fi
  mv -f "$3.part" "$3"
}

install_crate() { # line name version crate_sha patch_sha
  local line="$1" name="$2" version="$3" crate_sha="$4" patch_sha="$5"
  local dest="$crates/$name" mark="$crates/$name/.koru-fetched"
  if [ -f "$mark" ] && [ "$(cat "$mark")" = "$line" ]; then
    return 0
  fi

  local patch="$root/third-party/patches/$name.patch" have
  if [ ! -f "$patch" ]; then
    echo "[third-party] third-party/patches/$name.patch is missing" >&2
    return 1
  fi
  have="$(sha256 "$patch")"
  if [ "$have" != "$patch_sha" ]; then
    echo "[third-party] third-party/patches/$name.patch does not match its pin in fetch.manifest" >&2
    echo "[third-party]   pin  $patch_sha" >&2
    echo "[third-party]   file $have" >&2
    echo "[third-party] if the patch was changed on purpose, put the new sha256 in fetch.manifest" >&2
    return 1
  fi

  local crate="$downloads/$name-$version.crate"
  if [ ! -f "$crate" ] || [ "$(sha256 "$crate")" != "$crate_sha" ]; then
    get_verified "https://static.crates.io/crates/$name/$name-$version.crate" \
      "$crate_sha" "$crate" "$name $version from crates.io" || return 1
  fi

  rm -rf "$staging"
  mkdir -p "$staging"
  if ! tar -xzf "$crate" -C "$staging" || [ ! -f "$staging/$name-$version/Cargo.toml" ]; then
    echo "[third-party] could not unpack $name-$version.crate" >&2
    return 1
  fi

  # From the root, so the paths git sees are the same inside a checkout and
  # in a source tree that is not one.
  if ! (cd "$root" && git apply -p1 --whitespace=nowarn \
        --directory="third-party/crates/.staging/$name-$version" "third-party/patches/$name.patch"); then
    echo "[third-party] third-party/patches/$name.patch does not apply to $name $version" >&2
    return 1
  fi

  rm -rf "$dest"
  mv "$staging/$name-$version" "$dest"
  printf '%s\n' "$line" >"$mark"
  echo "[third-party] $name $version, patched, in third-party/crates/$name"
}

install_file() { # line path sha url [flags...]
  local line="$1" path="$2" sha="$3" url="$4"
  shift 4
  local dest="$root/$path" mark="$marks/${path//\//__}"
  if [ -f "$dest" ] && [ -f "$mark" ] && [ "$(cat "$mark")" = "$line" ]; then
    return 0
  fi
  mkdir -p "$(dirname "$dest")"
  get_verified "$url" "$sha" "$dest" "$path" || return 1
  local flag
  for flag in "$@"; do
    case "$flag" in
      # perl, not sed: BSD sed (macOS) has no \r.
      crlf) perl -pi -e 's/\r?\n/\r\n/' "$dest" ;;
      exec) chmod +x "$dest" ;;
    esac
  done
  printf '%s\n' "$line" >"$mark"
  echo "[third-party] placed $path"
}

failed=0
while IFS= read -r line || [ -n "$line" ]; do
  line="${line%$'\r'}"
  case "$line" in ''|'#'*) continue ;; esac
  read -r -a fields <<<"$line"
  case "${fields[0]}" in
    crate) install_crate "$line" "${fields[@]:1}" || failed=1 ;;
    file)  install_file "$line" "${fields[@]:1}" || failed=1 ;;
    *)     echo "[third-party] fetch.manifest: unknown entry '${fields[0]}'" >&2; failed=1 ;;
  esac
done <"$manifest"

rm -rf "$staging"
if [ "$failed" -ne 0 ]; then
  exit 1
fi
cp -f "$manifest" "$stamp"
