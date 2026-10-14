#!/usr/bin/env bash
# SPDX-License-Identifier: MPL-2.0

set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$repo"
common="$(git -C "$repo" rev-parse --path-format=absolute --git-common-dir)"
main_repo="$(dirname "$common")"
models="$main_repo/dist/voice-models"
./koru.sh voice-models fetch --work "$models" --only nemotron-3.5-streaming-0.6b-160ms
./koru.sh voice-models fetch --work "$models" --only whisper-base
./koru.sh voice-models local --work "$models"

serve="$models/serve"
catalog="$serve/voice/catalog-v1.json"
test -f "$catalog"

python3 -m http.server 8787 --bind 127.0.0.1 --directory "$serve" >/dev/null 2>&1 &
server_pid=$!
trap 'kill "$server_pid" 2>/dev/null || true' EXIT INT TERM

export KORU_VOICE_CATALOG="$catalog"
export KORU_VOICE_MODELS="$models/installed"
export KORU_ROOT="$repo/target/voice-test-root"
echo "[Koru] shared voice cache: $models"
echo "[Koru] voice test root: $KORU_ROOT"
echo "[Koru] Compact = Whisper; High Accuracy = Nemotron"
./koru.sh run-all
