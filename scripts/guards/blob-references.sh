#!/usr/bin/env bash
# FIX-223 (D-466): every column named `*blob_key` declared by a migration is a reference the
# orphan-blob sweep honours. The sweep (`blobs::delete_unreferenced`) deletes a blob nothing
# references; a column it does not know deletes that column's blobs an hour after upload (agent
# avatars did, until FIX-223). The reference list is `BLOB_REFERENCE_COLUMNS` in
# crates/aurora-db/src/repo/blobs.rs: a `("table", "column")` pair per line, each also compared
# with `b.key` in the query. A column that does not point into `blobs` goes in
# scripts/guards/blob-references.allow (`table.column  # reason`).
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

SWEEP=crates/aurora-db/src/repo/blobs.rs
ALLOW=scripts/guards/blob-references.allow

# "table column" pairs from CREATE TABLE bodies and ALTER TABLE ... ADD COLUMN.
pairs="$(awk '
  {
    line = $0
    sub(/--.*/, "", line)
    u = toupper(line)
    if (u ~ /^[ \t]*CREATE[ \t]+TABLE/ || u ~ /^[ \t]*ALTER[ \t]+TABLE/) {
      t = tolower(line)
      sub(/^[ \t]*(create|alter)[ \t]+table[ \t]+/, "", t)
      sub(/^if[ \t]+(not[ \t]+)?exists[ \t]+/, "", t)
      sub(/^only[ \t]+/, "", t)
      sub(/[^a-z_0-9].*/, "", t)
      cur = t
    }
    # A declaration: `name text|uuid|varchar|bytea` (a column definition or ADD COLUMN), not a use.
    if (match(line, /[a-z_0-9]*blob_key[ \t]+(text|uuid|varchar|bytea|TEXT|UUID|VARCHAR|BYTEA)/)) {
      c = substr(line, RSTART, RLENGTH)
      sub(/[ \t].*/, "", c)
      if (cur != "") print cur, c
    }
  }
' crates/aurora-db/migrations/*.sql | sort -u)"

if [ -z "$pairs" ]; then
  echo "blob-references guard: found no *blob_key column in the migrations (the scan is broken)" >&2
  exit 1
fi

fail=0
while read -r table col; do
  [ -n "$table" ] || continue
  if grep -qE "^${table}\.${col}([[:space:]]|$)" "$ALLOW" 2>/dev/null; then continue; fi
  if ! grep -qF "(\"${table}\", \"${col}\")" "$SWEEP"; then
    echo "blob-references guard: ${table}.${col} stores a blob key but is not in BLOB_REFERENCE_COLUMNS of $SWEEP." >&2
    echo "  Add it to the list and to the sweep's query (D-466), or list '${table}.${col}  # reason' in $ALLOW." >&2
    fail=1
  fi
done <<< "$pairs"

# The list and the query must agree: every listed column is compared with b.key in the SQL.
for col in $(grep -oE '\("[a-z_]+", "[a-z_]*blob_key"\)' "$SWEEP" | sed -E 's/.*, "([a-z_]*blob_key)".*/\1/' | sort -u); do
  if ! grep -qE "\.${col} = b\.key" "$SWEEP"; then
    echo "blob-references guard: $col is listed in BLOB_REFERENCE_COLUMNS but the sweep's query does not compare it with b.key" >&2
    fail=1
  fi
done

exit "$fail"
