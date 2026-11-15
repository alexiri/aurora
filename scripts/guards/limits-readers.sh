#!/usr/bin/env bash
# FIX-116 (REVIEW-5 LIM-x): every constant of `crates/aurora-core/src/limits.rs` has a reader
# outside the file (any Rust source: production or test) and a row in docs/02; backticked
# limit names in docs/02 exist in limits.rs. A constant nobody reads is a documented limit that
# is not enforced: wire it, or delete it together with its row.
#
# Known gaps are listed in `scripts/guards/limits-gaps.allow`, one `NAME tag` per line with
# tag `unread` (no reader), `no-row` (no docs/02 row) or `doc-only` (docs/02 names it, limits.rs
# does not). The list is a ratchet: it can only shrink — an entry whose gap has closed fails the
# guard until it is removed.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

LIMITS=crates/aurora-core/src/limits.rs
ALLOW=scripts/guards/limits-gaps.allow
DOC="$(ls docs/02-*.md | head -1)"

listed() { grep -qE "^$1[[:space:]]+$2([[:space:]]|$)" "$ALLOW" 2>/dev/null; }

# Every Rust file outside limits.rs, once (identifier search is a fixed-string word match).
corpus="$(mktemp)"
trap 'rm -f "$corpus"' EXIT
find crates services connectors -name '*.rs' -not -path '*/target/*' -not -path "./$LIMITS" \
  -not -path "$LIMITS" -print0 2>/dev/null | xargs -0 cat > "$corpus"

fail=0
check() { # name tag present(0/1) message
  local name="$1" tag="$2" ok="$3" msg="$4"
  if [ "$ok" = 0 ] && ! listed "$name" "$tag"; then
    echo "limits guard: $name $msg (fix it, or list '$name $tag' in $ALLOW)" >&2
    fail=1
  fi
  if [ "$ok" = 1 ] && listed "$name" "$tag"; then
    echo "limits guard: $name no longer has the '$tag' gap; remove it from $ALLOW" >&2
    fail=1
  fi
}

for n in $(grep -oE '^pub const [A-Z][A-Z0-9_]*' "$LIMITS" | awk '{print $3}'); do
  if grep -qw -e "$n" "$corpus"; then check "$n" unread 1 ""; else check "$n" unread 0 "has no reader outside $LIMITS"; fi
  if grep -qw -e "$n" "$DOC"; then check "$n" no-row 1 ""; else check "$n" no-row 0 "has no row in $DOC"; fi
done

for n in $(grep -oE '`[A-Z][A-Z0-9]*(_[A-Z0-9]+)+`' "$DOC" | tr -d '`' | sort -u); do
  if grep -qw "^pub const $n" "$LIMITS" || grep -qE "^pub const $n\b" "$LIMITS"; then
    check "$n" doc-only 1 ""
  else
    check "$n" doc-only 0 "is named in $DOC but is not a constant of $LIMITS"
  fi
done

exit "$fail"
