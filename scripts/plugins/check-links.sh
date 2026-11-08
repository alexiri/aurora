#!/usr/bin/env bash
# Link check for built-in packages: about.api_docs, about.api_spec, package.homepage and
# fields[].help_url must resolve. Needs the network, so it never runs in the blocking CI jobs:
# the `plugin-links` workflow (weekly + manual + PR, non-blocking) runs it.
#
#   bash scripts/plugins/check-links.sh [package-id...]
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"
dirs=()
if [ "$#" -gt 0 ]; then
  for p in "$@"; do dirs+=("plugins/builtin/$p"); done
else
  for d in plugins/builtin/*/; do dirs+=("${d%/}"); done
fi
exec python3 -I scripts/plugins/check_links.py "${dirs[@]}"
