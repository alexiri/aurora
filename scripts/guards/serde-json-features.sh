#!/usr/bin/env bash
# Architecture guard: serde_json's `arbitrary_precision` feature stays off in everything the
# gateway is built from. With it on, serde_json hands every number to a visitor as an object
# with one private member, so the strict reader of request bodies (aurora_policy::json) would
# read numbers as objects and no condition on a number would mean what it says. Any crate in the
# tree could switch it on for all of them: features are unified.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

tree=$(cargo tree --locked -p aurora-gateway -e features,no-dev -i serde_json)

# The tree lists the enabled features one by one; `std` is always there. Without it the output
# is not what this guard reads, and a silent pass would be worth nothing.
if ! grep -q 'serde_json feature "std"' <<<"$tree"; then
    echo "serde-json-features: FAIL — cargo tree lists no serde_json features (update the guard)" >&2
    exit 1
fi
if grep -q 'serde_json feature "arbitrary_precision"' <<<"$tree"; then
    echo "serde-json-features: FAIL — serde_json/arbitrary_precision is enabled in the gateway's dependency tree:" >&2
    grep -A6 'serde_json feature "arbitrary_precision"' <<<"$tree" >&2
    exit 1
fi
echo "serde-json-features: ok"
