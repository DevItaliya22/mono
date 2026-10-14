#!/bin/sh
# SPDX-License-Identifier: MPL-2.0

# Check out docs/, the koru-docs submodule, where this clone can reach it.
#
#   sh scripts/setup/setup-docs.sh          try once (setup)
#   sh scripts/setup/setup-docs.sh --hook   only in a clone that already has it
#
# **docs/ is the team's private documentation repository, and nothing needs
# it to build.** The code, the generated references (reference/) and the
# licensing audit (licensing/) are all in this repository; docs/ holds the
# product, design and planning pages. Someone without access gets an empty
# docs/ and a working build, so a failure here is a note, never an error.
#
# Credential prompts are turned off: without them a clone with no access
# would raise a login window every time this ran. The hook form also skips
# any clone where docs/ was never checked out, so `git worktree add` in a
# public clone does not try the network at all.
#
# Keep this in step with :ensure_docs in koru.bat.
set -u

root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$root"

grep -q 'path = docs' .gitmodules 2>/dev/null || exit 0
[ -f docs/README.md ] && exit 0

if [ "${1:-}" = "--hook" ]; then
  common="$(git rev-parse --git-common-dir 2>/dev/null)" || exit 0
  [ -d "$common/modules/docs" ] || exit 0
fi

if GIT_TERMINAL_PROMPT=0 GCM_INTERACTIVE=never \
   git -c credential.interactive=never submodule update --init docs >/dev/null 2>&1; then
  echo "[docs] checked out docs/ (koru-docs)"
else
  echo "[docs] docs/ is the team's private koru-docs repository and could not be checked out;" >&2
  echo "[docs]   the build does not need it. With access: git submodule update --init docs" >&2
  exit 1
fi
