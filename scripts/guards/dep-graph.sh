#!/usr/bin/env bash
# Architecture guard: forbidden dependency edges must not exist in the normal
# (non-dev, non-build) dependency graph.
#  - aurora-api must not reach wasmtime (runtime isolation, docs/90-decisions.md I-02)
#  - aurora-gateway must not reach wasmtime
#  - aurora-policy must not reach aurora-db (policy is pure logic, no persistence)
# If a package is absent from the graph, cargo reports "did not match any packages";
# that means the edge cannot exist and counts as a pass.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

fail=0

# check <from-package> <forbidden-package>
check() {
    local from="$1" forbidden="$2" out status
    set +e
    out="$(cargo tree -p "$from" -e normal -i "$forbidden" 2>&1)"
    status=$?
    set -e

    if [ "$status" -eq 0 ] && [ -n "$out" ]; then
        echo "dep-graph: FAIL — $from depends on $forbidden:" >&2
        echo "$out" >&2
        fail=1
    elif [ "$status" -ne 0 ] && grep -q "specification \`$forbidden\` did not match any packages" <<<"$out"; then
        echo "dep-graph: ok — $forbidden not in graph of $from"
    elif [ "$status" -eq 0 ]; then
        echo "dep-graph: ok — $forbidden not in graph of $from"
    else
        echo "dep-graph: ERROR running cargo tree for $from -> $forbidden:" >&2
        echo "$out" >&2
        fail=1
    fi
}

check aurora-api wasmtime
check aurora-gateway wasmtime
check aurora-policy aurora-db

if [ "$fail" -ne 0 ]; then
    echo "dep-graph: FAIL" >&2
    exit 1
fi
echo "dep-graph: OK"
