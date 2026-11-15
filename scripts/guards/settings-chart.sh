#!/usr/bin/env bash
# Instance settings and Helm locks (SET-1, D-237(c)): the chart renders a movable key into
# aurora.toml only when the operator set it, because a key present there is LOCKED (SEC-89). The
# guard renders the chart with default values (and with the relay, terminal and SCIM switches on)
# and fails when the rendered aurora.toml (any ConfigMap data key ending in `.toml`) contains a key
# of deploy/helm/aurora/movable-keys.txt. The list grows as each section brief registers its keys
# (a unit test in aurora-config keeps it equal to the settings registry). A planted key proves the
# check can fail (mutation self-test).
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

fail() { echo "settings-chart: FAIL: $*" >&2; exit 1; }

# shellcheck source=scripts/guards/lib.sh
source scripts/guards/lib.sh
require_helm settings-chart || exit 0

LIST=deploy/helm/aurora/movable-keys.txt
[ -f "$LIST" ] || fail "$LIST is missing"

# check <movable-keys file>  (rendered chart on stdin): prints the movable keys the rendered
# configuration sets, one per line.
check() {
  python3 -I -c '
import sys, tomllib, yaml

movable = {l.strip() for l in open(sys.argv[1]) if l.strip() and not l.startswith("#")}
found = set()

def walk(node, prefix):
    for k, v in node.items():
        path = f"{prefix}.{k}" if prefix else k
        if isinstance(v, dict):
            walk(v, path)
        else:
            found.add(path)
            # keyed lists: the list key itself counts (its entries are tables)

for doc in yaml.safe_load_all(sys.stdin):
    if not doc or doc.get("kind") != "ConfigMap":
        continue
    for name, text in (doc.get("data") or {}).items():
        if name.endswith(".toml"):
            walk(tomllib.loads(text), "")
for key in sorted(movable & found):
    print(key)
' "$1"
}

render() {
  helm template guard deploy/helm/aurora --set database.allowSharedLogin=true "$@"
}

scan() {
  local label="$1"; shift
  local hits
  hits="$(render "$@" | check "$LIST")" || fail "$label: the chart render failed"
  [ -z "$hits" ] || fail "$label: the rendered aurora.toml sets movable key(s) the operator did not set (each would count as locked, D-237(c)): $(echo "$hits" | tr '\n' ' ')"
}

scan "default values"
scan "relay, terminal and scim on" --set relay.enabled=true --set relay.domain=previews.guard.invalid \
  --set terminal.enabled=true --set scim.enabled=true

# --- mutation self-test: a planted movable key in the rendered configuration is reported ---
planted="$(mktemp)"
trap 'rm -f "$planted"' EXIT
echo "telemetry.log_level" >"$planted"
[ -n "$(render | check "$planted")" ] || fail "self-test: a movable key present in the rendered aurora.toml was not detected"
echo "hosted.nonexistent_key" >"$planted"
[ -z "$(render | check "$planted")" ] || fail "self-test: an absent key was reported"

echo "settings-chart: ok"
