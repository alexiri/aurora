#!/usr/bin/env bash
# SEC-91 (P5-GUIDE-2): the agent guide is advisory output, never an input. `tools.md` and the access lines
# of `guide.md` are written into the agent's home by the API; nothing that decides a request reads them
# back. The gateway, the policy engine and the hook host therefore must not mention `.aurora/apps`.
#
#   agent-guide-advisory.sh              scan the three crates (exit 1 on a reference)
#   agent-guide-advisory.sh --self-test  prove the scan fails on a planted reference, passes on clean
#                                        files, and fails on a missing directory (FIX-068: no vacuous pass)
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

# scan <dir>...: prints the references; exit 0 = none, 1 = found, 2 = a directory is missing or grep failed.
scan() {
  local d rc out
  for d in "$@"; do
    [ -d "$d" ] || { echo "agent-guide-advisory: FAIL: scan directory $d is missing" >&2; return 2; }
  done
  set +e
  out="$(grep -rIFn --exclude-dir=target --exclude-dir=node_modules -e '.aurora/apps' -e 'aurora/apps/' "$@")"
  rc=$?
  set -e
  case "$rc" in
    0) printf '%s\n' "$out" >&2; return 1 ;;
    1) return 0 ;;
    *) echo "agent-guide-advisory: FAIL: grep failed (exit $rc)" >&2; return 2 ;;
  esac
}

if [ "${1:-}" = "--self-test" ]; then
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' EXIT
  mkdir -p "$tmp/clean" "$tmp/planted"
  echo 'fn main() {}' > "$tmp/clean/a.rs"
  echo 'let p = ".aurora/apps/gitlab/tools.md";' > "$tmp/planted/a.rs"
  scan "$tmp/clean" || { echo "agent-guide-advisory: self-test FAIL: clean directory rejected" >&2; exit 1; }
  if scan "$tmp/clean" "$tmp/planted" 2>/dev/null; then
    echo "agent-guide-advisory: self-test FAIL: planted reference not found" >&2; exit 1
  fi
  if scan "$tmp/clean" "$tmp/missing" 2>/dev/null; then
    echo "agent-guide-advisory: self-test FAIL: missing directory passed" >&2; exit 1
  fi
  echo "agent-guide-advisory: self-test ok"
  exit 0
fi

scan services/aurora-gateway crates/aurora-policy services/aurora-hookhost \
  || { echo "agent-guide-advisory: FAIL (SEC-91): the gateway, policy and hook host must not read the agent guide" >&2; exit 1; }
echo "agent-guide-advisory: ok"
