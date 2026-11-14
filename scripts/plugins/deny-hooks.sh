#!/usr/bin/env bash
# cargo-deny over every built-in hook crate (P2-OPS-1, D-100). The plugins/builtin/*/hooks crates are
# standalone projects with their own Cargo.lock, so the workspace `cargo deny check` never sees them.
# Each crate carries its own deny.toml (the github one holds a scoped RUSTSEC-2023-0071 waiver).
# Also fails if a hook crate has no deny.toml, so a new hook cannot slip past the scan.
#
#   bash scripts/plugins/deny-hooks.sh
# Needs cargo-deny (CI: taiki-e/install-action). Advisory DB access needs network (cached after the first run).
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

if ! cargo deny --version >/dev/null 2>&1; then
  echo "deny-hooks: cargo-deny is not installed (cargo install cargo-deny --locked)" >&2
  exit 2
fi

fail=0
n=0
for h in plugins/builtin/*/hooks; do
  [ -f "$h/Cargo.toml" ] || continue
  n=$((n + 1))
  if [ ! -f "$h/deny.toml" ]; then
    echo "deny-hooks: $h has no deny.toml" >&2
    fail=1
    continue
  fi
  echo "== cargo deny: $h"
  if ! cargo deny --manifest-path "$h/Cargo.toml" --locked --config "$h/deny.toml" check -A license-not-encountered advisories licenses bans sources; then
    fail=1
  fi
done
[ "$n" -gt 0 ] || { echo "deny-hooks: no hook crates found" >&2; exit 2; }
[ "$fail" -eq 0 ] || { echo "deny-hooks: FAILED" >&2; exit 1; }
echo "deny-hooks: $n hook crates clean"
