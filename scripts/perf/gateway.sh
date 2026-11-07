#!/usr/bin/env bash
# Gateway performance budgets (docs/18 §Performance budgets), OPS-014. Release mode.
#   (a) added latency, cached allowed bodyless request with injection: p50 < 1 ms, p99 < 5 ms
#   (b) >= 2000 requests/s on ONE gateway core, cached path (load test, taskset-pinned)
#   (c) connect-time resolution on a cache miss, 1000 rules / 500 connections: p99 < 25 ms
# Prints PASS/FAIL per budget; exit 1 if any fails. Env: PERF_RUNS (default 3, median reported),
# PERF_LOAD_SECS (10), PERF_LOAD_CONNS (32), PERF_LATENCY_N, PERF_RESOLVE_N, PERF_CORES (cpu list
# the whole run may use; default: all online CPUs).
set -uo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"
RUNS="${PERF_RUNS:-3}"
export PERF_LOAD_SECS="${PERF_LOAD_SECS:-10}"

echo "load average: $(cut -d' ' -f1-3 /proc/loadavg) ($(nproc) CPUs)"
timeout 540 cargo build --release -p aurora-gateway --bench gateway_perf >&2 || { echo "build failed" >&2; exit 1; }
BIN=$(ls -t target/release/deps/gateway_perf-* 2>/dev/null | grep -v '\.d$' | head -1)
[ -x "$BIN" ] || { echo "bench binary not found" >&2; exit 1; }

# CPU plan: gateway on one core, upstream on one, load generator on two, picked from the
# allowed set; falls back to sharing when fewer CPUs exist.
mapfile -t CPUS < <(taskset -cp $$ | sed 's/.*: //' | tr ',' '\n' | awk -F- '{if(NF==2){for(i=$1;i<=$2;i++)print i}else print $1}')
n=${#CPUS[@]}
GW_CPU=${CPUS[$((n - 1))]}
UP_CPU=${CPUS[$((n > 1 ? n - 2 : 0))]}
LG_CPU="${CPUS[$((n > 3 ? n - 4 : 0))]},${CPUS[$((n > 2 ? n - 3 : 0))]}"

median() { sort -n | awk '{a[NR]=$1} END{print a[int((NR+1)/2)]}'; }
field() { grep '^RESULT' | sed -n "s/.*$1=\([0-9.]*\).*/\1/p"; }

run_a=(); run_c=(); run_b=()
for i in $(seq "$RUNS"); do
  out=$("$BIN" latency 2>&1); echo "[a run $i] $(echo "$out" | grep -v "RESULT\|^client:" | head -1) reconnects=$(echo "$out" | field reconnects)"
  echo "$out" | grep -q '^RESULT' || { echo "$out" >&2; exit 1; }
  run_a+=("$(echo "$out" | field a_added_p50_ms) $(echo "$out" | field a_added_p99_ms)")
  out=$("$BIN" resolve 2>&1); echo "[c run $i] $(echo "$out" | grep -v RESULT | head -1)"
  echo "$out" | grep -q '^RESULT' || { echo "$out" >&2; exit 1; }
  run_c+=("$(echo "$out" | field c_p99_ms)")
done

UP_LOG=$(mktemp); GW_LOG=$(mktemp)
cleanup() { [ -n "${UP_PID:-}" ] && kill "$UP_PID" 2>/dev/null; [ -n "${GW_PID:-}" ] && kill "$GW_PID" 2>/dev/null; rm -f "$UP_LOG" "$GW_LOG"; }
trap cleanup EXIT
taskset -c "$UP_CPU" "$BIN" upstream >"$UP_LOG" 2>&1 & UP_PID=$!
for _ in $(seq 50); do grep -q '^ADDR' "$UP_LOG" && break; sleep 0.1; done
UP=$(sed -n 's/^ADDR //p' "$UP_LOG")
taskset -c "$GW_CPU" "$BIN" gateway "$UP" >"$GW_LOG" 2>&1 & GW_PID=$!
for _ in $(seq 50); do grep -q '^ADDR' "$GW_LOG" && break; sleep 0.1; done
GW=$(sed -n 's/^ADDR //p' "$GW_LOG")
[ -n "$UP" ] && [ -n "$GW" ] || { echo "gateway/upstream did not start" >&2; exit 1; }
echo "gateway pid $GW_PID pinned to cpu $GW_CPU (affinity: $(taskset -cp $GW_PID | sed 's/.*: //')), upstream cpu $UP_CPU, load generator cpus $LG_CPU"
for i in $(seq "$RUNS"); do
  out=$(taskset -c "$LG_CPU" "$BIN" load "$GW" "$UP" 2>&1); echo "[b run $i] $(echo "$out" | grep -v "RESULT\|^client:" | head -1) reconnects=$(echo "$out" | field reconnects)"
  echo "$out" | grep -q '^RESULT' || { echo "$out" >&2; exit 1; }
  [ "$(echo "$out" | field b_non200)" = 0 ] || { echo "load run saw non-200 responses" >&2; exit 1; }
  run_b+=("$(echo "$out" | field b_rps)")
done

A50=$(printf '%s\n' "${run_a[@]}" | awk '{print $1}' | median)
A99=$(printf '%s\n' "${run_a[@]}" | awk '{print $2}' | median)
B=$(printf '%s\n' "${run_b[@]}" | median)
C=$(printf '%s\n' "${run_c[@]}" | median)
fail=0
verdict() { # name value op limit unit
  if awk -v v="$2" -v l="$4" -v op="$3" 'BEGIN{exit !((op=="<" && v<l) || (op==">=" && v>=l))}'; then r=PASS; else r=FAIL; fail=1; fi
  echo "$r  $1: $2 $5 (budget $3 $4 $5, median of $RUNS runs)"
}
echo
verdict "(a) added latency p50" "$A50" "<" 1 ms
verdict "(a) added latency p99" "$A99" "<" 5 ms
verdict "(b) throughput, 1 gateway core" "$B" ">=" 2000 req/s
verdict "(c) resolve cache-miss p99, 1000 rules/500 conns" "$C" "<" 25 ms
exit $fail
