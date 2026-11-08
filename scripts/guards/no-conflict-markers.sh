#!/usr/bin/env bash
# No merge-conflict markers in tracked files. A lane's own "merge master into the branch" once
# committed a lone diff3 base marker (`||||||| <rev>`) into docs/90-decisions.md; scanning only
# for `<<<<<<<`/`>>>>>>>` missed it. Lines that consist of a marker (optionally followed by a
# space and a label) are refused; Markdown setext underlines of `=` never have exactly seven.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

hits=$(git grep -nIE '^(<{7}|\|{7}|={7}|>{7})( .*)?$' -- . ':!*.lock' || true)
if [ -n "$hits" ]; then
  echo "no-conflict-markers: FAIL: merge-conflict markers in tracked files:" >&2
  printf '%s\n' "$hits" >&2
  exit 1
fi
echo "no-conflict-markers: ok"
