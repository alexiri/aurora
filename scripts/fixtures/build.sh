#!/usr/bin/env bash
# Phase 0 fixture packages (docs/09 §13, FIX-001): build the hooks guest, pack + sign every
# fixture into plugins/fixtures/dist/*.aurora-plugin, run `aurora plugin validate` on each
# (invalid ones must fail with the code listed in plugins/fixtures/expected-invalid.txt) and
# `aurora plugin test` on the valid ones.
#
# Env: AURORA_BIN / AURORA_GATEWAY_BIN (default target/debug), SKIP_CARGO=1 to skip cargo builds.
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
FIX="$ROOT/plugins/fixtures"
DIST="$FIX/dist"
STAGE="$DIST/stage"
KEY="$FIX/test-key/test-signing-key"
PUB="$FIX/test-key/test-signing-key.pub"
PY="python3 -I $ROOT/scripts/fixtures/mkfixture.py"
AURORA="${AURORA_BIN:-$ROOT/target/debug/aurora}"
export AURORA_GATEWAY_BIN="${AURORA_GATEWAY_BIN:-$ROOT/target/debug/aurora-gateway}"

if [ -z "${SKIP_CARGO:-}" ]; then
  (cd "$ROOT" && cargo build -q -p aurora-cli -p aurora-gateway) || { echo "FATAL: cargo build failed"; exit 2; }
fi
rm -rf "$DIST"; mkdir -p "$STAGE"

# Guest component (standalone cargo project, not a workspace member).
HOOKS_WASM="$DIST/hooks-target/wasm32-wasip2/release/fixture_hooks.wasm"
(cd "$FIX/fixture-hooks/hooks" && CARGO_TARGET_DIR="$DIST/hooks-target" RUSTFLAGS= \
  cargo build -q --release --target wasm32-wasip2) || { echo "FATAL: guest build failed"; exit 2; }

# A second key that the test configuration does not trust.
"$AURORA" plugin keygen --out "$DIST/untrusted-key" >/dev/null || exit 2

fail=0; pass=0; xfail=0
note() { printf '%-34s %s\n' "$1" "$2"; }

pack() { # <name> -> dist/<name>.aurora-plugin ; honours the `mutate` file
  local name="$1" src="$FIX/$1" out="$DIST/$1.aurora-plugin" mode wasm=""
  # The CLI refuses to build a hooks manifest without hooks.wasm, so every package that
  # declares hooks gets the fixture guest; `drop-wasm` removes it again afterwards.
  grep -Eq '^  hooks: \[.+\]' "$src/plugin.yaml" && wasm="$HOOKS_WASM"
  mode="$(cat "$src/mutate" 2>/dev/null || true)"
  $PY stage "$src" "$STAGE/$name" $wasm || return 1
  case "$mode" in
    unsigned) "$AURORA" plugin build "$STAGE/$name" --out "$out" >/dev/null ;;
    untrusted) "$AURORA" plugin build "$STAGE/$name" --out "$out" --key "$DIST/untrusted-key" >/dev/null ;;
    *) "$AURORA" plugin build "$STAGE/$name" --out "$out" --key "$KEY" >/dev/null ;;
  esac || return 1
  case "$mode" in
    tamper|symlink|traversal|unknown-path|duplicate) $PY mutate "$out" "$mode" ;;
    drop-wasm) $PY mutate "$out" "$mode" && "$AURORA" plugin sign "$out" --key "$KEY" >/dev/null ;;
  esac
}

json() { python3 -I -c "import sys,json; d=json.load(sys.stdin); print($1)"; }

for src in "$FIX"/fixture-*/; do
  name="$(basename "$src")"
  case "$name" in fixture-invalid-*) continue ;; esac
  if ! pack "$name"; then note "$name" "FAIL (pack)"; fail=$((fail+1)); continue; fi
  pkg="$DIST/$name.aurora-plugin"
  v="$("$AURORA" plugin validate "$pkg" --pubkey "$PUB" 2>&1)"
  if ! printf '%s' "$v" | json 'd["ok"]' 2>/dev/null | grep -qx True; then
    note "$name" "FAIL (validate)"; printf '%s\n' "$v"; fail=$((fail+1)); continue
  fi
  # `plugin test` has no --pubkey and rejects signed archives (CLI gap), so it runs on the
  # staged source directory, which is packed unsigned in memory.
  t="$("$AURORA" plugin test "$STAGE/$name" 2>&1)"
  summary="$(printf '%s' "$t" | json '"%s pass=%d fail=%d skipped=%d" % (d["ok"], d["fixtures"]["passed"], d["fixtures"]["failed"], d["fixtures"]["skipped"])' 2>/dev/null)"
  if printf '%s' "$summary" | grep -q '^True'; then
    note "$name" "ok   $summary"; pass=$((pass+1))
  else
    note "$name" "FAIL (test)"; printf '%s\n' "$t"; fail=$((fail+1))
  fi
done

# Invalid packages: `<name> <expected code> [xfail]`
while read -r name code flag; do
  case "$name" in ''|'#'*) continue ;; esac
  if [ ! -d "$FIX/$name" ]; then note "$name" "FAIL (no source dir)"; fail=$((fail+1)); continue; fi
  if ! pack "$name"; then note "$name" "FAIL (pack)"; fail=$((fail+1)); continue; fi
  out="$("$AURORA" plugin validate "$DIST/$name.aurora-plugin" --pubkey "$PUB" 2>&1)"; rc=$?
  if [ $rc -ne 0 ] && printf '%s' "$out" | grep -Eq "(\"code\": \"$code\"| $code at )"; then
    note "$name" "ok   rejected: $code"; pass=$((pass+1))
  elif [ "${flag:-}" = xfail ]; then
    note "$name" "XFAIL (known tooling gap) expected $code, rc=$rc"; xfail=$((xfail+1))
  else
    note "$name" "FAIL expected $code, rc=$rc"; printf '%s\n' "$out"; fail=$((fail+1))
  fi
done < "$FIX/expected-invalid.txt"

echo "--- passed=$pass failed=$fail xfail=$xfail"
[ "$fail" -eq 0 ]
