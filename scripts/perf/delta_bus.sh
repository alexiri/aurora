#!/usr/bin/env bash
# `notify` DeltaBus throughput and latency (docs/15 §Scaling, "Streaming fan-out ceiling", RFE-016).
# Release mode. Measures publish -> receive of 2 000-char deltas over Postgres NOTIFY, against a
# THROWAWAY database created next to $DATABASE_URL (default: read from ./.env) and dropped at the
# end; the shared tables and the shared `aurora_delta` channel are never used. The role needs
# CREATEDB.
#   paced: 1 000 notifications/s (the comfortable region): publish p99 < 50 ms, nothing lost
#   sat:   closed-loop saturation: delivered >= 2 000 notifications/s (low end of the 2 000-3 000/s ceiling)
# Prints PASS/WARN per planning number; WARN (slow machine, busy Postgres) never fails the script.
# Exit 1 only when the measurement itself could not run. Env: DATABASE_URL, PERF_SECS (5 per
# phase), PERF_PUBLISHERS (16, saturation phase).
set -uo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"
if [ -z "${DATABASE_URL:-}" ] && [ -f .env ]; then
  DATABASE_URL="$(sed -n 's/^DATABASE_URL=//p' .env | head -1)"
fi
[ -n "${DATABASE_URL:-}" ] || { echo "DATABASE_URL is not set and .env has none" >&2; exit 1; }
export DATABASE_URL

echo "load average: $(cut -d' ' -f1-3 /proc/loadavg) ($(nproc) CPUs)"
timeout 540 cargo build --release -p aurora-db --bench delta_bus_perf >&2 || { echo "build failed" >&2; exit 1; }
BIN=$(ls -t target/release/deps/delta_bus_perf-* 2>/dev/null | grep -v '\.d$' | head -1)
[ -x "$BIN" ] || { echo "bench binary not found" >&2; exit 1; }

OUT=$(mktemp)
cleanup() {
  # If the bench died before dropping its throwaway database, drop it here.
  db=$(sed -n 's/^throwaway database: //p' "$OUT" | head -1)
  if [ -n "$db" ] && ! grep -q "^dropped $db" "$OUT"; then
    psql "$DATABASE_URL" -qc "DROP DATABASE IF EXISTS $db WITH (FORCE)" >&2 || echo "could not drop $db" >&2
  fi
  rm -f "$OUT"
}
trap cleanup EXIT
"$BIN" 2>&1 | tee "$OUT" | grep -v '^RESULT'
grep -q '^RESULT paced_' "$OUT" && grep -q '^RESULT sat_' "$OUT" || { echo "measurement did not complete" >&2; exit 1; }

field() { grep '^RESULT' "$OUT" | tr ' ' '\n' | sed -n "s/^$1=//p" | head -1; }
verdict() { # name value op limit unit
  if awk -v v="$2" -v l="$4" -v op="$3" 'BEGIN{exit !((op=="<" && v<l) || (op==">=" && v>=l) || (op=="==" && v==l))}'; then r=PASS; else r=WARN; fi
  echo "$r  $1: $2 $5 (planning number $3 $4 $5)"
}
echo
verdict "paced 1000/s: publish p99" "$(field paced_publish_p99_ms)" "<" 50 ms
verdict "paced 1000/s: notifications lost" "$(field paced_lost)" "==" 0 notifications
verdict "paced 1000/s: publish->receive p99 (informational, no planning number)" "$(field paced_deliver_p99_ms)" "<" 50 ms
verdict "saturation: delivered throughput" "$(field sat_delivered_per_s)" ">=" 2000 notifications/s
echo "saturation: publish p99 $(field sat_publish_p99_ms) ms, lost $(field sat_lost) (docs/15: beyond ~2000-3000/s commit latency of all writers rises)"
exit 0
