#!/usr/bin/env bash
# SEC-91 (P5-GUIDE-2): the agent guide is advisory output, never an input. `tools.md` and the access lines
# of `guide.md` are written into the agent's home by the API; nothing that decides a request reads them
# back. The gateway, the policy engine and the hook host therefore must not mention `.aurora/apps`.
#
# SEC-150 (P5-APPSKILL-1/2): the same holds for the skills of an app package. The gateway, the policy
# engine, the hook host and the runner never read `skills/` from a package nor the snapshot's
# `agent.skills`: nothing that decides or runs a request depends on skill content or scripts.
#
#   agent-guide-advisory.sh              scan the crates (exit 1 on a reference)
#   agent-guide-advisory.sh --self-test  prove the scan fails on a planted reference, passes on clean
#                                        files, and fails on a missing directory (FIX-068: no vacuous pass)
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

# scan <dir>...: prints the references; exit 0 = none, 1 = found, 2 = a directory is missing or grep failed.
# The patterns: the agent's folder (SEC-91) and the package skills (SEC-150: the archive directory
# constant, the shared validator, the snapshot's agent files).
scan() {
  local d rc out
  for d in "$@"; do
    [ -d "$d" ] || { echo "agent-guide-advisory: FAIL: scan directory $d is missing" >&2; return 2; }
  done
  set +e
  out="$(grep -rIFn --exclude-dir=target --exclude-dir=node_modules -e '.aurora/apps' -e 'aurora/apps/' \
    -e 'package::SKILLS_DIR' -e 'skill_format' -e 'snapshot::SkillEntry' -e 'agent_files' "$@")"
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
  mkdir -p "$tmp/planted2"
  echo 'use aurora_registry::skill_format;' > "$tmp/planted2/a.rs"
  scan "$tmp/clean" || { echo "agent-guide-advisory: self-test FAIL: clean directory rejected" >&2; exit 1; }
  if scan "$tmp/clean" "$tmp/planted" 2>/dev/null; then
    echo "agent-guide-advisory: self-test FAIL: planted reference not found" >&2; exit 1
  fi
  if scan "$tmp/clean" "$tmp/planted2" 2>/dev/null; then
    echo "agent-guide-advisory: self-test FAIL: planted skill reference not found" >&2; exit 1
  fi
  if scan "$tmp/clean" "$tmp/missing" 2>/dev/null; then
    echo "agent-guide-advisory: self-test FAIL: missing directory passed" >&2; exit 1
  fi
  echo "agent-guide-advisory: self-test ok"
  exit 0
fi

scan services/aurora-gateway crates/aurora-policy services/aurora-hookhost services/aurora-runner \
  || { echo "agent-guide-advisory: FAIL (SEC-91, SEC-150): the gateway, policy, hook host and runner must not read the agent folder or package skills" >&2; exit 1; }
echo "agent-guide-advisory: ok"
