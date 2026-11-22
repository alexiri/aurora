#!/usr/bin/env bash
# Tests the `grafana-query` script that the built-in grafana package ships in its skill
# (plugins/builtin/grafana/skills/grafana/grafana-query/scripts), with no network: a stub
# `aurora-agent` first on PATH records its arguments and the request bodies and answers canned files
# from scripts/plugins/fixtures/grafana/. The canned frames follow the shapes of Grafana's
# POST /api/ds/query answer (documented in its HTTP API reference, public/openapi3.json of v13.3.1:
# results.<refId>.{status, error, errorSource, frames[].{schema.fields[].labels, data.values}}); the
# values are made up. Needs bash, jq and python3.
#
#   bash scripts/plugins/grafana_skills_test.sh
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRIPT="$ROOT/plugins/builtin/grafana/skills/grafana/grafana-query/scripts/grafana-query"
FIX="$ROOT/scripts/plugins/fixtures/grafana"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
export TMPDIR="$WORK/tmp"
mkdir -p "$TMPDIR" "$WORK/bin" "$WORK/bodies"
export STUB_FIX="$FIX" STUB_LOG="$WORK/calls.log" STUB_BODIES="$WORK/bodies"
unset STUB_EMPTY

# The stub: `aurora-agent call grafana <operation> [name=value ...] [--body @file] [--account A] --out FILE`.
cat > "$WORK/bin/aurora-agent" <<'STUB'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "$STUB_LOG"
[ "${1:-}" = call ] && [ "${2:-}" = grafana ] || { echo "stub: not a grafana call: $*" >&2; exit 2; }
op=$3; shift 3
out=""; body=""
while [ $# -gt 0 ]; do
  case "$1" in
    --out) out=$2; shift ;;
    --body) body=${2#@}; shift ;;
  esac
  shift
done
[ -n "$out" ] || { echo "stub: --out missing" >&2; exit 2; }
case "$out" in /*) ;; *) echo "stub: --out must be absolute: $out" >&2; exit 2 ;; esac
case "$op" in
  list_datasources) if [ -n "${STUB_EMPTY:-}" ]; then echo '[]' > "$out"; else cp "$STUB_FIX/list_datasources.json" "$out"; fi ;;
  get_dashboard) cp "$STUB_FIX/get_dashboard.json" "$out" ;;
  query_datasources)
    n=$(find "$STUB_BODIES" -type f | wc -l)
    cp "$body" "$STUB_BODIES/body-$((n + 1)).json"
    if grep -q '"expr": "bad' "$body"; then cat "$STUB_FIX/query_error.txt" >&2; exit 1
    elif grep -q '"instant": true\|"queryType": "instant"' "$body"; then cp "$STUB_FIX/query_instant.json" "$out"
    elif grep -q 'raw_data' "$body"; then cp "$STUB_FIX/query_raw.json" "$out"
    elif grep -q 'os-logs' "$body"; then cp "$STUB_FIX/query_opensearch.json" "$out"
    else cp "$STUB_FIX/query_range.json" "$out"; fi ;;
  *) echo "stub: no fixture for $op" >&2; exit 1 ;;
esac
STUB
chmod +x "$WORK/bin/aurora-agent"
export PATH="$WORK/bin:$PATH"

fails=0
ok() { echo "ok   $1"; }
bad() { echo "FAIL $1" >&2; fails=$((fails + 1)); }
expect_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1"; echo "--- want" >&2; echo "$2" >&2; echo "--- got" >&2; echo "$3" >&2; fi; }
expect_has() { if grep -qF -- "$2" <<<"$3"; then ok "$1"; else bad "$1: missing '$2' in:"; echo "$3" >&2; fi; }
body() { cat "$WORK/bodies/body-$1.json"; }
reset() { : > "$STUB_LOG"; rm -f "$WORK"/bodies/*; }
G() { "$SCRIPT" "$@"; }

[ -x "$SCRIPT" ] && ok "script is executable" || bad "script is not executable"
python3 -I -m py_compile "$SCRIPT" && ok "py_compile" || bad "py_compile"
find "$ROOT/plugins/builtin/grafana/skills" -name __pycache__ -prune -exec rm -rf {} + 2>/dev/null || true

# --- datasources -------------------------------------------------------------------------------------
out=$(G datasources)
want=$'prom-main\tprometheus\tMain Prometheus\nloki-main\tloki\tMain Loki\nos-logs\tgrafana-opensearch-datasource\tLogs?[31m'
expect_eq "datasources: uid, type, name only; control characters replaced" "$want" "$out"
if grep -q "prom.internal\|secureJsonFields" <<<"$out"; then bad "datasources: printed more than uid, type, name"; else ok "datasources: nothing else printed"; fi
expect_eq "datasources: --type" $'loki-main\tloki\tMain Loki' "$(G datasources --type loki)"
out=$(STUB_EMPTY=1 G datasources)
expect_has "datasources: an empty list says so and suggests panels" "grafana-query panels" "$out"
expect_has "datasources: an empty list explains the permissions" "depends on the token's data source permissions" "$out"
reset; G datasources --account B >/dev/null
expect_has "datasources: --account is passed through" "--account B" "$(cat "$STUB_LOG")"

# --- panels ---------------------------------------------------------------------------------------------
out=$(G panels api-overview)
expect_has "panels: variable data source resolved" "[1] Request rate (timeseries) datasource: prom-main (variable datasource) prometheus" "$out"
expect_has "panels: expr" 'A: expr=sum(rate(http_requests_total{job="$job"}[5m]))' "$out"
expect_has "panels: a panel inside a row, OpenSearch target" "[3] Errors by host (table) datasource: os-logs grafana-opensearch-datasource" "$out"
expect_has "panels: metrics, bucket aggregations and time field" "query=level:error | metrics=count: | bucketAggs=terms:host.keyword,date_histogram:@timestamp | timeField=@timestamp" "$out"
expect_has "panels: a target with its own data source and SQL" "A [pg-reports postgres]: rawSql=SELECT count(*) FROM deployments" "$out"
expect_has "panels: a string \${var} data source resolved" "[4] Rows in table (stat) datasource: prom-main (variable datasource)" "$out"
if grep -q "Logs row" <<<"$out"; then bad "panels: printed the row itself"; else ok "panels: rows are expanded, not listed"; fi
if G panels 'x; rm -rf /' 2>/dev/null; then bad "panels: accepted a bad uid"; else ok "panels: bad uid refused"; fi

# --- promql: body, range frames --------------------------------------------------------------------
reset
out=$(G promql --ds prom-main --from 2026-11-20T13:00:00Z --to 2026-11-20T15:00:00Z --step 5m 'sum(up)' 'rate(x[5m])')
from_ms=$(date -u -d 2026-11-20T13:00:00Z +%s)000
expect_eq "promql: from converted to epoch ms" "$from_ms" "$(body 1 | jq -r .from)"
expect_eq "promql: refIds, data source, expressions" 'A prom-main prometheus sum(up)|B prom-main prometheus rate(x[5m])' \
  "$(body 1 | jq -r '[.queries[] | "\(.refId) \(.datasource.uid) \(.datasource.type) \(.expr)"] | join("|")')"
expect_eq "promql: intervalMs from --step" "300000" "$(body 1 | jq -r '.queries[0].intervalMs')"
expect_eq "promql: maxDataPoints from the window (24 points + 1)" "25" "$(body 1 | jq -r '.queries[0].maxDataPoints')"
expect_eq "promql: a range query" "true null" "$(body 1 | jq -r '"\(.queries[0].range) \(.queries[0].instant)"')"
expect_eq "promql: one call for both expressions" 1 "$(grep -c query_datasources "$STUB_LOG")"
expect_has "promql: call goes through aurora-agent query_datasources" "call grafana query_datasources --body @" "$(cat "$STUB_LOG")"
t0=$(date -u -d @1763643600 +%FT%T.000Z)
expect_has "promql: refId status line" "# A 200" "$out"
expect_has "promql: frame header with labels" $'Time\tValue{job=api,status=200}' "$out"
expect_has "promql: times as RFC 3339 UTC, values" "$t0"$'\t10.5' "$out"
expect_has "promql: small values keep digits" $'\t0.000123457' "$out"
expect_has "promql: a null value is empty" $'\t' "$out"
expect_has "promql: second series" $'Time\tValue{job=api,status=500}' "$out"

# --- promql: instant ------------------------------------------------------------------------------------
reset
out=$(G promql --ds prom-main --from now-1h --to now --instant 'sum by (job) (up)')
expect_eq "promql --instant: instant true, no range" "true null" "$(body 1 | jq -r '"\(.queries[0].instant) \(.queries[0].range)"')"
expect_has "promql --instant: one point per series" $'Time\tValue{job=api}' "$out"
expect_has "promql --instant: both series printed" $'Time\tValue{job=web}' "$out"

# --- logql ------------------------------------------------------------------------------------------------
reset
G logql --ds loki-main --from now-1h --to now --step 1m --limit 50 '{app="api"} |= "error"' >/dev/null
expect_eq "logql: type, queryType, maxLines, step" 'loki range 50 60s' \
  "$(body 1 | jq -r '.queries[0] | "\(.datasource.type) \(.queryType) \(.maxLines) \(.step)"')"

# --- the point limit --------------------------------------------------------------------------------------
reset
rc=0; err=$(G promql --ds prom-main --from now-30d --to now --step 15s 'up' 2>&1) || rc=$?
expect_eq "points limit: refused with a usage exit" 2 "$rc"
expect_has "points limit: names a step that fits" "use --step 4m" "$err"
expect_eq "points limit: nothing was sent" 0 "$(grep -c query_datasources "$STUB_LOG" || true)"
rc=0; G promql --ds prom-main --from now-1h --to now --step 0s up >/dev/null 2>&1 || rc=$?
expect_eq "bad step refused" 2 "$rc"
rc=0; G promql --ds prom-main --from yesterday --to now up >/dev/null 2>&1 || rc=$?
expect_eq "bad time refused" 2 "$rc"
rc=0; G promql --ds 'a b' --from now-1h --to now up >/dev/null 2>&1 || rc=$?
expect_eq "bad data source uid refused" 2 "$rc"

# --- a per-refId error: non-zero exit, the good refId still printed ---------------------------------------
reset
rc=0; out=$(G promql --ds prom-main --from now-1h --to now up 'bad_query(' 2>&1) || rc=$?
expect_eq "refId error: exit status 1" 1 "$rc"
expect_has "refId error: says it retries alone" "trying each refId alone" "$out"
expect_has "refId error: the good refId is printed" "# A 200" "$out"
expect_has "refId error: the failed refId is named with the error" "# B failed: HTTP 400 from the app" "$out"
expect_has "refId error: errorSource shown" "errorSource" "$out"
expect_eq "refId error: one combined call and two single ones" 3 "$(grep -c query_datasources "$STUB_LOG")"

# --- run with an OpenSearch terms body ------------------------------------------------------------------------
cat > "$WORK/os.json" <<'JSON'
{"from": "now-12h", "to": "now", "queries": [{"refId": "A", "datasource": {"uid": "os-logs", "type": "grafana-opensearch-datasource"},
 "queryType": "lucene", "query": "level:error", "metrics": [{"id": "1", "type": "count"}],
 "bucketAggs": [{"id": "2", "type": "terms", "field": "host.keyword", "settings": {"size": "5", "orderBy": "_count", "order": "desc"}},
                {"id": "3", "type": "date_histogram", "field": "@timestamp", "settings": {"interval": "12h"}}]}]}
JSON
reset
out=$(G run "$WORK/os.json")
expect_has "run: status line" "# A 200" "$out"
expect_has "run: terms bucket as labels" $'Time\tValue{host.keyword=host-1}' "$out"
expect_has "run: bucket rows, one per date_histogram bucket" $'\t120' "$out"
expect_eq "run: the body is sent unchanged" "$(jq -S . "$WORK/os.json")" "$(body 1 | jq -S .)"
echo '{"from":"now-1h","to":"now","queries":[{"refId":"A"},{"refId":"A"}]}' > "$WORK/dup.json"
rc=0; G run "$WORK/dup.json" >/dev/null 2>&1 || rc=$?
expect_eq "run: duplicate refIds refused" 2 "$rc"
echo '{"from":"now-1h","to":"now","queries":[]}' > "$WORK/none.json"
rc=0; G run "$WORK/none.json" >/dev/null 2>&1 || rc=$?
expect_eq "run: no queries refused" 2 "$rc"

# --- frames -------------------------------------------------------------------------------------------------------
out=$(G frames "$FIX/query_instant.json")
expect_has "frames: prints a saved answer" $'Time\tValue{job=web}' "$out"
echo '{"results":{"A":{"status":400,"error":"boom","errorSource":"plugin","frames":[]}}}' > "$WORK/err.json"
rc=0; out=$(G frames "$WORK/err.json") || rc=$?
expect_eq "frames: an error refId exits 1" 1 "$rc"
expect_has "frames: error shown" "# A 400 error: boom (errorSource plugin)" "$out"

# --- --columns -------------------------------------------------------------------------------------------------------
RAW="$FIX/query_raw.json"
out=$(G frames "$RAW")
expect_eq "frames without --columns: every column" \
  $'metadata.timestamp\t_id\t_index\tdata.clientip\tdata.path\tdata.user_agent\tdata.agent' "$(sed -n 3p <<<"$out")"
out=$(G frames "$RAW" --columns clientip,path)
expect_eq "frames --columns: time field and the two named fields" $'metadata.timestamp\tdata.clientip\tdata.path' "$(sed -n 3p <<<"$out")"
expect_eq "frames --columns: rows have three columns" "3 3 3" \
  "$(sed -n '4,6p' <<<"$out" | awk -F'\t' '{printf "%s%s", sep, NF; sep=" "}')"
expect_has "frames --columns: row values" $'\t192.0.2.2\t/b' "$out"
if grep -q "no field matches" <<<"$out"; then bad "frames --columns: a matching name printed the no-match line"; else ok "frames --columns: no no-match line when all names match"; fi
out=$(G frames "$RAW" --columns agent)
expect_eq "frames --columns agent: data.agent, not data.user_agent" $'metadata.timestamp\tdata.agent' "$(sed -n 3p <<<"$out")"
out=$(G frames "$RAW" --columns ip)
expect_eq "frames --columns ip: only the time field" "metadata.timestamp" "$(grep -v '^#\|^##' <<<"$out" | sed -n 1p)"
expect_has "frames --columns ip: names the miss and lists the fields" "# --columns: no field matches ip; fields: metadata.timestamp, _id, _index, data.clientip" "$out"
out=$(G frames "$RAW" --columns 'path,,clientip')
expect_eq "frames --columns: empty names are ignored, frame order kept" $'metadata.timestamp\tdata.clientip\tdata.path' "$(sed -n 3p <<<"$out")"
cat > "$WORK/raw.json" <<'JSON'
{"from": "now-1h", "to": "now", "queries": [{"refId": "A", "datasource": {"uid": "os-logs", "type": "grafana-opensearch-datasource"},
 "queryType": "lucene", "query": "*", "metrics": [{"id": "1", "type": "raw_data", "settings": {"size": "3", "order": "desc"}}]}]}
JSON
reset
out=$(G run "$WORK/raw.json" --columns clientip,path)
expect_has "run --columns: status line" "# A 200" "$out"
expect_has "run --columns: selection applied" $'metadata.timestamp\tdata.clientip\tdata.path' "$out"
expect_eq "run --columns: the body is sent unchanged" "$(jq -S . "$WORK/raw.json")" "$(body 1 | jq -S .)"
out=$(G run "$WORK/raw.json")
expect_has "run without --columns: every column" $'\tdata.user_agent\tdata.agent' "$out"
for v in ',' ',,' ' '; do
  rc=0; G frames "$RAW" --columns "$v" >/dev/null 2>&1 || rc=$?
  expect_eq "--columns '$v' is a usage error" 2 "$rc"
done
rc=0; G run "$WORK/raw.json" --columns , >/dev/null 2>&1 || rc=$?
expect_eq "run --columns , is a usage error" 2 "$rc"
long=$(printf 'a%.0s' $(seq 1 101))
rc=0; G frames "$RAW" --columns "$long" >/dev/null 2>&1 || rc=$?
expect_eq "--columns: a name over 100 characters is a usage error" 2 "$rc"
many=$(seq -s, 1 51)
rc=0; G frames "$RAW" --columns "$many" >/dev/null 2>&1 || rc=$?
expect_eq "--columns: more than 50 names is a usage error" 2 "$rc"

# --- hygiene -------------------------------------------------------------------------------------------------------
if grep -nE '\bcurl\b|\bwget\b|requests|urllib|http\.client|import socket|eval\(|exec\(|shell=True|os\.system|GRAFANA_|TOKEN|Authorization' "$SCRIPT"; then
  bad "the script uses the network, a shell or credentials directly"
else
  ok "script: aurora-agent only, no shell, no credentials"
fi
if [ -n "$(find "$TMPDIR/aurora-agent" -maxdepth 1 -name 'grafana-query-*' 2>/dev/null)" ]; then bad "temp files left behind"; else ok "temp files removed"; fi

if [ "$fails" -ne 0 ]; then echo "$fails check(s) failed" >&2; exit 1; fi
echo "grafana-query: all checks passed"
