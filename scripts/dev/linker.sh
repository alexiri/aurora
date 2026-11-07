#!/bin/sh
# OPS-016: linker driver used by .cargo/config.toml for x86_64-linux.
# Uses clang + lld when both are installed (the dev host), otherwise falls back to the system `cc`,
# so CI runners and containers without lld keep building (just with the default linker).
#   AURORA_LD=lld     (default) clang -fuse-ld=lld
#   AURORA_LD=mold    clang -fuse-ld=mold (if the owner installed mold)
#   AURORA_LD=system  plain cc
#   AURORA_LINK_LOG=f append "<seconds> <linker> <output>" per link to f (for measuring)
set -eu
want="${AURORA_LD:-lld}"
if [ "$want" != system ] && command -v clang >/dev/null 2>&1 && command -v "ld.$want" >/dev/null 2>&1; then
  drv="clang"; set -- "-fuse-ld=$want" "$@"
else
  want=system; drv="cc"
fi
if [ -n "${AURORA_LINK_LOG:-}" ]; then
  start=$(date +%s.%N 2>/dev/null || date +%s)
  "$drv" "$@"; rc=$?
  end=$(date +%s.%N 2>/dev/null || date +%s)
  out=""; prev=""; for a in "$@"; do [ "$prev" = "-o" ] && out="$a"; prev="$a"; done
  echo "$(echo "$end - $start" | bc 2>/dev/null || echo "$end $start") $want $(basename "$out")" >> "$AURORA_LINK_LOG"
  exit $rc
fi
exec "$drv" "$@"
