#!/usr/bin/env bash
# FIX-098 (D-161): the Terminal tab serves a vendored copy of the Ghostty WASM from web/public. It
# is named by the package version (`ghostty-vt-<version>.wasm`) and its sha256 is recorded in
# scripts/guards/ghostty-wasm.sha256. This guard fails when
#   1. the vendored file's sha256 differs from the recorded value (an edit or a bad merge),
#   2. the file name's version differs from the pinned `@wterm/ghostty` version in web/package.json, or
#   3. (when web/node_modules is installed) the recorded value differs from the pinned package's own
#      wasm/ghostty-vt.wasm — i.e. the package was bumped and the vendored copy was not refreshed.
# To refresh after a bump: copy node_modules/@wterm/ghostty/wasm/ghostty-vt.wasm to
# web/public/ghostty-vt-<version>.wasm, update WASM_URL in terminal-tab.tsx, record the new sha256.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

record=scripts/guards/ghostty-wasm.sha256
fail() { echo "ghostty-wasm: FAIL: $*" >&2; exit 1; }

[ -f "$record" ] || fail "$record is missing"
read -r want file < "$record"
[ -f "$file" ] || fail "$file (named by $record) is missing"

got=$(sha256sum "$file" | cut -d' ' -f1)
[ "$got" = "$want" ] || fail "$file sha256 is $got, recorded $want"

pin=$(grep -o '"@wterm/ghostty": *"[^"]*"' web/package.json | sed 's/.*: *"\(.*\)"/\1/' || true)
[ -n "$pin" ] || fail "@wterm/ghostty is not pinned in web/package.json"
case "$file" in
  *"ghostty-vt-$pin.wasm") ;;
  *) fail "$file does not carry the pinned @wterm/ghostty version $pin in its name" ;;
esac

pkg=web/node_modules/@wterm/ghostty/wasm/ghostty-vt.wasm
if [ -f "$pkg" ]; then
  pkg_sum=$(sha256sum "$pkg" | cut -d' ' -f1)
  [ "$pkg_sum" = "$want" ] || fail "the pinned package's wasm has sha256 $pkg_sum, the vendored copy $want: refresh the vendored file"
else
  echo "ghostty-wasm: note: $pkg not installed, skipped the package comparison" >&2
fi
echo "ghostty-wasm: ok ($file, $want)"
