#!/usr/bin/env bash
# Architecture guard: forbidden dependency edges must not exist in the normal
# (non-dev, non-build) dependency graph.
#  - aurora-api must not reach wasmtime (runtime isolation, docs/90-decisions.md I-02)
#  - aurora-gateway must not reach wasmtime
#  - aurora-policy must not reach aurora-db (policy is pure logic, no persistence)
#  - aurora-api and aurora-hookhost must not reach pg_query (the gateway database lane is its only user)
#  - aurora-hookhost must not reach aurora-db/-secrets/sqlx (SEC-13; aurora-crypto is linked only for signature verification via aurora-registry)
# If a package is absent from the graph, cargo reports "did not match any packages";
# that means the edge cannot exist and counts as a pass.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

fail=0

# (FIX-101) "did not match any packages" used to pass for ANY name, so a misspelt forbidden crate was a
# permanent green. Both packages must exist: `from` as a workspace member, `forbidden` in Cargo.lock
# (it is in the graph of some other crate; if the dependency is gone for good, delete the check).
lock_has() { grep -qx "name = \"$1\"" Cargo.lock; }

# check <from-package> <forbidden-package>
check() {
    local from="$1" forbidden="$2" out status
    if ! lock_has "$from"; then
        echo "dep-graph: FAIL — $from is not a package in Cargo.lock (misspelt, or removed: update the guard)" >&2
        fail=1; return
    fi
    if ! lock_has "$forbidden"; then
        echo "dep-graph: FAIL — $forbidden is not in Cargo.lock, so '$from must not reach it' proves nothing (misspelt, or the dependency is gone: update the guard)" >&2
        fail=1; return
    fi
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
# SEC-13: the hook host holds no keys and no database credentials
check aurora-hookhost aurora-db
check aurora-hookhost aurora-secrets
check aurora-hookhost sqlx
# P2-DBL-1: libpg_query (C) is linked by the gateway's database lane only
check aurora-api pg_query
check aurora-hookhost pg_query
check aurora-sqlclass aurora-db

if [ "$fail" -ne 0 ]; then
    echo "dep-graph: FAIL" >&2
    exit 1
fi
# Self-test (FIX-101): a misspelt forbidden crate must be refused, not counted as "not in the graph".
if ! (check aurora-api no-such-crate-planted 2>&1) | grep -q "is not in Cargo.lock"; then
    echo "dep-graph: FAIL — self-test: a misspelt forbidden crate was accepted" >&2
    exit 1
fi
echo "dep-graph: OK"
