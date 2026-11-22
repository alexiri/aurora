#!/usr/bin/env bash
# Tests the three scripts that the built-in grafana package ships in its skills: `grafana-query`
# (grafana-query/scripts), `grafana-os` (grafana-opensearch/scripts) and `grafana-prom`
# (grafana-prometheus/scripts; one script per skill because of the 32000-character skill limit), with no network: a stub
# `aurora-agent` first on PATH records its arguments and the request bodies and answers canned files
# from scripts/plugins/fixtures/grafana/. The canned frames follow the shapes of Grafana's
# POST /api/ds/query answer (documented in its HTTP API reference, public/openapi3.json of v13.3.1:
# results.<refId>.{status, error, errorSource, frames[].{schema.fields[].labels, data.values}}); the
# values are made up. Needs bash, jq and python3.
#
#   bash scripts/plugins/grafana_skills_test.sh
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SKILLS="$ROOT/plugins/builtin/grafana/skills/grafana"
SCRIPT="$SKILLS/grafana-query/scripts/grafana-query"
OSCRIPT="$SKILLS/grafana-opensearch/scripts/grafana-os"
PSCRIPT="$SKILLS/grafana-prometheus/scripts/grafana-prom"
FIX="$ROOT/scripts/plugins/fixtures/grafana"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
export TMPDIR="$WORK/tmp"
mkdir -p "$TMPDIR" "$WORK/bin" "$WORK/bodies"
export STUB_FIX="$FIX" STUB_LOG="$WORK/calls.log" STUB_BODIES="$WORK/bodies"
unset STUB_EMPTY STUB_FAIL_CALL STUB_SWEEP_ERR

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
    if grep -q '"expr": "bad' "$body" || [ "${STUB_FAIL_CALL:-}" = "$((n + 1))" ]; then cat "$STUB_FIX/query_error.txt" >&2; exit 1
    elif grep -q 'node_filesystem_size_bytes' "$body"; then
      if [ -n "${STUB_SWEEP_ERR:-}" ]; then cp "$STUB_FIX/query_sweep_err.json" "$out"; else cp "$STUB_FIX/query_sweep.json" "$out"; fi
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
O() { "$OSCRIPT" "$@"; }
PR() { "$PSCRIPT" "$@"; }

for s in "$SCRIPT" "$OSCRIPT" "$PSCRIPT"; do
  if [ -x "$s" ]; then ok "$(basename "$s") is executable"; else bad "$(basename "$s") is not executable"; fi
  if python3 -I -m py_compile "$s"; then ok "py_compile $(basename "$s")"; else bad "py_compile $(basename "$s")"; fi
done
find "$SKILLS" -name __pycache__ -prune -exec rm -rf {} + 2>/dev/null || true
# The scripts each carry their own copy of the small helpers (a skill may hold at most 32000
# characters, so no shared module): the copies must stay identical.
if python3 -I - "$SCRIPT" "$OSCRIPT" "$PSCRIPT" <<'PY'
import ast, sys
files = dict(zip(("query", "os", "prom"), sys.argv[1:]))
want = {
    "query,os,prom": ["clean", "die", "tmp_dir", "mktemp", "parse_step", "norm_time"],
    "query,prom": ["agent", "fit_step", "read_json", "send_body"],
}
consts = {"query,os,prom": ["CTRL", "UID_RE", "UNIT_S", "TIMEOUT"], "query,prom": ["POINTS_MAX"]}
src = {k: open(p, encoding="utf-8").read() for k, p in files.items()}
tree = {k: ast.parse(s) for k, s in src.items()}
bad = 0
def seg(k, name):
    for n in tree[k].body:
        if isinstance(n, ast.FunctionDef) and n.name == name:
            return ast.get_source_segment(src[k], n)
        if isinstance(n, ast.Assign) and any(getattr(t, "id", None) == name for t in n.targets):
            return ast.get_source_segment(src[k], n)
    return None
for group, names in {**want, **{g + "#c": c for g, c in consts.items()}}.items():
    keys = group.split("#")[0].split(",")
    for name in names:
        segs = [seg(k, name) for k in keys]
        if None in segs or len(set(segs)) != 1:
            print(f"helper {name} differs or is missing among {keys}", file=sys.stderr)
            bad += 1
sys.exit(1 if bad else 0)
PY
then ok "the helper functions copied into the three scripts are identical"; else bad "the copied helper functions have drifted apart"; fi

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
expect_has "run: a frame with a display name is named by it (moved with the display-name rule)" $'Time\thost-1' "$out"
expect_has "run: a frame without a display name keeps name and labels" $'Time\tValue{host.keyword=host-2}' "$out"
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

# --- display names (config.displayNameFromDS) --------------------------------------------------------------------
NAMES="$FIX/query_names.json"
out=$(G frames "$NAMES")
expect_has "display names: a Value field is named by the display name (Count)" $'Time\thost-1 Count' "$out"
expect_has "display names: two Value frames of one term differ (Sum bytes)" $'Time\thost-1 Sum bytes' "$out"
expect_has "display names: no config prints name and labels as before" $'Time\tValue{job=api}' "$out"
expect_has "display names: a named field keeps name{labels} and adds the display name" $'Time\tcount{host=h9} [h9 Count]' "$out"
if grep -q 'Value{host.keyword' <<<"$out"; then bad "display names: labels printed next to a display name on a Value field"; else ok "display names: Value with a display name has no labels"; fi

# --- --wide -------------------------------------------------------------------------------------------------------------
WIDE="$FIX/query_wide.json"
t1=$(date -u -d @1763643600 +%FT%T.000Z); t2=$(date -u -d @1763647200 +%FT%T.000Z); t3=$(date -u -d @1763650800 +%FT%T.000Z)
out=$(G frames "$WIDE" --wide)
expect_has "wide: one table for three single-series frames" "## wide 3 series" "$out"
expect_has "wide: Time and one column per series, named by display name" $'Time\th1 Count\th2 Count\th3 Count' "$out"
expect_has "wide: row 1, a missing point is -" "$t1"$'\t1\t10\t-' "$out"
expect_has "wide: row 2" "$t2"$'\t2\t-\t200' "$out"
expect_has "wide: row 3" "$t3"$'\t3\t30\t-' "$out"
expect_eq "wide: rows are sorted and distinct" 3 "$(grep -c '^20' <<<"$out")"
expect_eq "wide: the table is first and the table frame prints after it" "## wide 3 series|## frame table" \
  "$(grep '^##' <<<"$out" | paste -sd'|')"
expect_has "wide: the other frame prints as before" $'host.keyword\tCount\tSum bytes' "$out"
out=$(G frames "$WIDE")
if grep -q '## wide' <<<"$out"; then bad "wide: printed a wide table without --wide"; else ok "wide: off by default"; fi
out=$(G frames "$WIDE" --wide --max-rows 2)
expect_has "wide --max-rows: the table is cut with the total" "... 1 more rows not shown (3 in all)" "$out"

# --- --max-rows -----------------------------------------------------------------------------------------------------
out=$(G frames "$FIX/query_range.json" --max-rows 2)
expect_eq "max-rows 2: two rows of each of the two frames" 4 "$(grep -c '^20' <<<"$out")"
expect_has "max-rows 2: the cut line has the total" "... 1 more rows not shown (3 in all)" "$out"
out=$(G frames "$FIX/query_range.json" --max-rows 3)
if grep -q 'more rows' <<<"$out"; then bad "max-rows 3: cut a frame of three rows"; else ok "max-rows 3: nothing cut"; fi
for v in 0 5001 -1 x; do
  rc=0; G frames "$FIX/query_range.json" --max-rows "$v" >/dev/null 2>&1 || rc=$?
  expect_eq "--max-rows $v is a usage error" 2 "$rc"
done
rc=0; G frames "$FIX/query_range.json" --max-rows 5000 >/dev/null 2>&1 || rc=$?
expect_eq "--max-rows 5000 is accepted" 0 "$rc"
reset
out=$(G promql --ds prom-main --from now-1h --to now --max-rows 1 up)
expect_has "promql --max-rows applies to its frames" "... 2 more rows not shown (3 in all)" "$out"

# --- instant queries need only --to -------------------------------------------------------------------------------
reset
to_ms=$(date -u -d 2026-11-20T15:00:00Z +%s)000
G promql --ds prom-main --to 2026-11-20T15:00:00Z --instant 'sum(up)' >/dev/null
expect_eq "promql --instant without --from: from is to - 300000 ms" "$((to_ms - 300000))" "$(body 1 | jq -r .from)"
expect_eq "promql --instant without --from: to is sent as given" "$to_ms" "$(body 1 | jq -r .to)"
reset
G logql --ds loki-main --to 2026-11-20T15:00:00Z --instant 'count_over_time({app="api"}[5m])' >/dev/null
expect_eq "logql --instant without --from: from is to - 300000 ms" "$((to_ms - 300000))" "$(body 1 | jq -r .from)"
reset
G promql --ds prom-main --from 2026-11-20T16:00:00Z --to 2026-11-20T15:00:00Z --instant 'sum(up)' >/dev/null
expect_eq "promql --instant with --from after --to: replaced the same way" "$((to_ms - 300000))" "$(body 1 | jq -r .from)"
rc=0; G promql --ds prom-main --to now up >/dev/null 2>&1 || rc=$?
expect_eq "a range query without --from is a usage error" 2 "$rc"

# --- os ----------------------------------------------------------------------------------------------------------------------
reset
from_ms=$(date -u -d 2026-11-20T13:00:00Z +%s)000
OSA=(--ds os-logs --from 2026-11-20T13:00:00Z --to 2026-11-20T15:00:00Z)
out=$(O "${OSA[@]}" --query 'agent:"X/1.0"' --metric count --metric sum:bytes --terms host:5:sum:bytes --hist 30m --print-body)
want=$(jq -nS --arg f "$from_ms" --arg t "$to_ms" '{from: $f, to: $t, queries: [{refId: "A",
  datasource: {uid: "os-logs", type: "grafana-opensearch-datasource"}, queryType: "lucene", query: "agent:\"X/1.0\"",
  metrics: [{id: "1", type: "count"}, {id: "2", type: "sum", field: "bytes"}],
  bucketAggs: [{id: "3", type: "terms", field: "host", settings: {size: "5", orderBy: "2", order: "desc"}},
               {id: "4", type: "date_histogram", settings: {interval: "30m"}}],
  intervalMs: 1800000, maxDataPoints: 100}]}')
expect_eq "os: the exact body (query intact, orderBy = the sum's id, nesting order, no time field)" "$want" "$(body 1 | jq -S .)"
expect_has "os --print-body: prints the body first" '# body: {"from":"'"$from_ms"'"' "$out"
expect_has "os --print-body: the query string is intact in the printed body" 'agent:\"X/1.0\"' "$out"
expect_has "os: the answer is printed" "# A 200" "$out"
expect_eq "os: one call" 1 "$(grep -c query_datasources "$STUB_LOG")"
reset
O "${OSA[@]}" --terms host.keyword:3:_key --terms path --hist 1h --time-field metadata.timestamp >/dev/null
expect_eq "os: default metric count, _key ascending, default size 10, nesting, the given time field" \
  '[{"id":"1","type":"count"}]|[{"id":"2","type":"terms","field":"host.keyword","settings":{"size":"3","orderBy":"_key","order":"asc"}},{"id":"3","type":"terms","field":"path","settings":{"size":"10","orderBy":"_count","order":"desc"}},{"id":"4","type":"date_histogram","settings":{"interval":"1h"},"field":"metadata.timestamp"}]' \
  "$(body 1 | jq -c '.queries[0] | .metrics, .bucketAggs' | paste -sd'|')"
expect_eq "os: the interval goes to intervalMs" 3600000 "$(body 1 | jq -r '.queries[0].intervalMs')"
reset
O "${OSA[@]}" --metric percentiles:latency:50,99.9 --metric cardinality:ip --terms host --hist 12h --ds-type elasticsearch >/dev/null
expect_eq "os: percentiles with their list, cardinality, ds type" \
  'percentiles latency 50,99.9|cardinality ip|elasticsearch' \
  "$(body 1 | jq -r '.queries[0] | "\(.metrics[0].type) \(.metrics[0].field) \(.metrics[0].settings.percents | join(","))|\(.metrics[1].type) \(.metrics[1].field)|\(.datasource.type)"')"
reset
err=$(O "${OSA[@]}" --metric percentiles:latency --terms host 2>&1 >/dev/null)
expect_has "os: percentiles under terms alone: a note on stderr" "null percentiles" "$err"
expect_eq "os: the query still ran" 1 "$(grep -c query_datasources "$STUB_LOG")"
expect_eq "os: default percentiles" "50 95 99" "$(body 1 | jq -r '.queries[0].metrics[0].settings.percents | join(" ")')"
err=$(O "${OSA[@]}" --metric percentiles:latency --terms host --hist 12h 2>&1 >/dev/null)
if [ -n "$err" ]; then bad "os: a note although --hist is given: $err"; else ok "os: no note with --hist"; fi
reset
out=$(O "${OSA[@]}" --raw 3 --columns path)
expect_eq "os --raw: a raw_data metric of size 3, newest first, no buckets" 'raw_data|3|desc|null' \
  "$(body 1 | jq -r '.queries[0] | "\(.metrics[0].type)|\(.metrics[0].settings.size)|\(.metrics[0].settings.order)|\(.bucketAggs)"')"
expect_has "os --raw --columns: the selection applies" $'metadata.timestamp\tdata.path' "$out"
os_usage() { # a usage error and nothing sent; the arguments follow the fixed ones
  reset
  rc=0; O "${OSA[@]}" "$@" >/dev/null 2>&1 || rc=$?
  expect_eq "os: usage error for $*" 2 "$rc"
  expect_eq "os: nothing sent for $*" 0 "$(grep -c query_datasources "$STUB_LOG" || true)"
}
os_usage --metric median:x --terms host
os_usage --metric sum --terms host
os_usage --metric count:x --terms host
os_usage --metric percentiles:f:0 --terms host
os_usage --metric percentiles:f:101 --terms host
os_usage --metric percentiles:f:1,2,3,4,5,6,7,8,9,10,11 --terms host
os_usage --metric sum:bytes --terms host:5:avg:bytes
os_usage --terms host:0
os_usage --terms host:10001
os_usage --terms host:5:nonsense
os_usage --metric count
os_usage --hist 5x
os_usage --raw 0
os_usage --raw 201
os_usage --raw 3 --terms host
os_usage --raw 3 --metric count
os_usage --raw 3 --hist 1h
os_usage --terms 'host.keyword"' --hist 1h
os_usage --terms 'a b'
os_usage --metric 'sum:a"b' --terms host
os_usage --terms a:1 --terms b:1 --terms c:1 --terms d:1
os_usage --metric count --metric count --metric count --metric count --metric count --metric count --metric count --terms h
os_usage --time-field 'a"b' --hist 1h
os_usage --ds-type loki --hist 1h
os_usage --query ' ' --hist 1h
rc=0; O --ds 'a b' --from now-1h --to now --hist 1h >/dev/null 2>&1 || rc=$?
expect_eq "os: bad data source uid refused" 2 "$rc"
reset
O "${OSA[@]}" --query '*" }], "x": "y' --hist 1h >/dev/null
expect_eq "os: a query with quotes and braces stays one JSON string" '*" }], "x": "y' "$(body 1 | jq -r '.queries[0].query')"
expect_eq "os: and adds no key" "refId datasource queryType query metrics bucketAggs intervalMs maxDataPoints" \
  "$(body 1 | jq -r '.queries[0] | keys_unsorted | join(" ")')"

# --- compare -------------------------------------------------------------------------------------------------------------
reset
out=$(PR compare --ds prom-main --from 2026-11-20T13:00:00Z --to 2026-11-20T15:00:00Z --step 5m 'sum(up)')
expect_eq "compare: one call per window (now, 1d, 7d)" 3 "$(grep -c query_datasources "$STUB_LOG")"
expect_eq "compare: the windows shift by exactly 1 d and 7 d" "$from_ms $to_ms|$((from_ms - 86400000)) $((to_ms - 86400000))|$((from_ms - 604800000)) $((to_ms - 604800000))" \
  "$(for i in 1 2 3; do body "$i" | jq -r '"\(.from) \(.to)"'; done | paste -sd'|')"
expect_eq "compare: a range query at the step, the expression" "true 300000 sum(up)" "$(body 2 | jq -r '.queries[0] | "\(.range) \(.intervalMs) \(.expr)"')"
expect_has "compare: names the expression" "# expr: sum(up)" "$out"
expect_has "compare: names the step" "# step 300s" "$out"
expect_has "compare: names each window exactly (now)" "# window now: 2026-11-20T13:00:00Z .. 2026-11-20T15:00:00Z" "$out"
expect_has "compare: names each window exactly (1d)" "# window 1d: 2026-11-19T13:00:00Z .. 2026-11-19T15:00:00Z" "$out"
expect_has "compare: names each window exactly (7d)" "# window 7d: 2026-11-13T13:00:00Z .. 2026-11-13T15:00:00Z" "$out"
expect_has "compare: the stats row of a series" $'job=api,status=200\tnow\t0.000123457\t10.5\t10.95\t11\t7.16671\t3' "$out"
expect_has "compare: a row for each offset window" $'job=api,status=200\t7d\t' "$out"
expect_has "compare: the p95 ratio line" $'job=api,status=200\tp95 now / median p95 before\t1x' "$out"
reset
rc=0; out=$(STUB_FAIL_CALL=2 PR compare --ds prom-main --from 2026-11-20T13:00:00Z --to 2026-11-20T15:00:00Z 'sum(up)' 2>&1) || rc=$?
expect_eq "compare: a failed window exits 1" 1 "$rc"
expect_has "compare: the failed window prints its error" "# window 1d failed: HTTP 400 from the app" "$out"
expect_has "compare: the other windows still print" $'job=api,status=200\t7d\t' "$out"
rc=0; PR compare --ds prom-main --from now-2h --to now --offsets 1x up >/dev/null 2>&1 || rc=$?
expect_eq "compare: a bad offset is a usage error" 2 "$rc"
rc=0; PR compare --ds prom-main --from now-2h --to now --offsets 1d,2d,3d,4d,5d,6d up >/dev/null 2>&1 || rc=$?
expect_eq "compare: more than 5 offsets is a usage error" 2 "$rc"
reset
PR compare --ds prom-main --from now-2h --to now --offsets 2d 'sum(up)' >/dev/null
expect_eq "compare: --offsets 2d is two calls" 2 "$(grep -c query_datasources "$STUB_LOG")"

# --- sweep -----------------------------------------------------------------------------------------------------------------
SEL='job="node",instance=~"web.*"'
SW=(sweep --ds prom-main --selector "$SEL" --from 2026-11-20T03:00:00Z --to 2026-11-20T15:00:00Z)
reset
out=$(PR "${SW[@]}")
expect_eq "sweep: every query in one query_datasources call" 1 "$(grep -c query_datasources "$STUB_LOG")"
expect_eq "sweep: six default checks and four filesystem queries" "C0 C1 C2 C3 C4 C5 FSIZE FFREE FMIN FAGO" \
  "$(body 1 | jq -r '[.queries[].refId] | join(" ")')"
expect_eq "sweep: instant queries of the prometheus type at --to" "true prometheus $to_ms" \
  "$(body 1 | jq -r '"\(.queries[0].instant) \(.queries[0].datasource.type) \(.to)"')"
expect_eq "sweep: \${SELECTOR}, \${WINDOW} and \${HOST} substituted" \
  "min by (instance) (min_over_time(up{$SEL}[43200s]))" "$(body 1 | jq -r '.queries[0].expr')"
expect_eq "sweep: no placeholder left in any expression" false "$(body 1 | jq -r '[.queries[].expr | contains("${")] | any')"
expect_has "sweep: the filesystem queries exclude pseudo filesystems" 'fstype!~"tmpfs|overlay|squashfs|devtmpfs|autofs"' "$(body 1 | jq -r '.queries[6].expr')"
expect_has "sweep: the trend is an offset" "offset 7d" "$(body 1 | jq -r '.queries[9].expr')"
expect_has "sweep: host x check table, short host names as columns" $'check\tflag\tweb-1\tweb-2' "$out"
expect_has "sweep: a value under its flag has no !" $'up min\t<1\t1\t1' "$out"
expect_has "sweep: a crossed flag has !" $'load1 max / cpu\t>2\t0.5\t!2.5' "$out"
expect_has "sweep: another crossed flag" $'memory used max\t>0.9\t0.4\t!0.95' "$out"
expect_eq "sweep: a network filesystem on two hosts is one row" 1 "$(grep -c '/shared' <<<"$out")"
expect_has "sweep: ! on under 10 % free" $'!/shared\tceph\t1000\t20\t2\t19\t0\t-' "$out"
expect_has "sweep: ! on under 30 days to full (linear)" $'!/\text4\t100\t60\t60\t59\t5.71429\t10.5' "$out"
expect_has "sweep: no ! on a healthy filesystem" $'\n/data\txfs\t500\t300\t60\t299\t4.28571\t70' $'\n'"$out"
rc=0; PR "${SW[@]}" >/dev/null || rc=$?
expect_eq "sweep: exit 0 when nothing failed" 0 "$rc"
reset
out=$(PR "${SW[@]}" --show-queries)
expect_has "sweep --show-queries: the window" "# window 2026-11-20T03:00:00Z .. 2026-11-20T15:00:00Z" "$out"
expect_has "sweep --show-queries: every expression" "#   C0 (up min): min by (instance) (min_over_time(up{$SEL}[43200s]))" "$out"
expect_has "sweep --show-queries: the filesystem expressions" "#   FAGO: min by (device, mountpoint, fstype) (node_filesystem_avail_bytes{" "$out"
reset
out=$(PR "${SW[@]}" --json --show-queries)
if jq -e '.window and (.filesystems | length == 3) and (.errors | length == 0) and (.checks["load1 max / cpu"]["web-2.example.org:9100"] == 2.5) and .queries.C0' <<<"$out" >/dev/null; then ok "sweep --json: parses with jq and has window, checks, filesystems, errors"; else bad "sweep --json: unexpected JSON"; echo "$out" >&2; fi
reset
PR sweep --ds prom-main --selector "$SEL" --last 12h --host-label host >/dev/null
expect_eq "sweep --last 12h: a window of 12 hours" 43200000 "$(body 1 | jq -r '(.to | tonumber) - (.from | tonumber)')"
expect_eq "sweep --host-label: used for the grouping" "min by (host) (min_over_time(up{$SEL}[43200s]))" "$(body 1 | jq -r '.queries[0].expr')"
# placeholders: exactly three tokens, nothing else rewritten, nothing else accepted
cat > "$WORK/checks.json" <<'JSON'
{"checks": [{"name": "busy workers", "expr": "max by (${HOST}) (workers_busy{${SELECTOR}}[${WINDOW}]) + HOST_total + $SELECTOR{HOST=\"x\", WINDOW=\"y\"} + SELECTOR", "flag": ">=300"}]}
JSON
reset
out=$(PR "${SW[@]}" --checks "$WORK/checks.json")
expect_eq "sweep: the file's check is added after the defaults" "C6" "$(body 1 | jq -r '.queries[6].refId')"
expect_eq "sweep: only the three tokens are replaced; HOST, WINDOW, SELECTOR and \$SELECTOR without braces are kept" \
  "max by (instance) (workers_busy{$SEL}[43200s]) + HOST_total + \$SELECTOR{HOST=\"x\", WINDOW=\"y\"} + SELECTOR" \
  "$(body 1 | jq -r '.queries[6].expr')"
expect_has "sweep: a file check's row with its flag" $'busy workers\t>=300\t12\t!345' "$out"
for tok in '${SEL}' '${selector}' '${HOST }' '${}' '${FOO}'; do
  jq -n --arg e "up{\${SELECTOR}} + $tok" '{checks: [{name: "x", expr: $e}]}' > "$WORK/ph.json"
  reset
  rc=0; err=$(PR "${SW[@]}" --checks "$WORK/ph.json" 2>&1 >/dev/null) || rc=$?
  expect_eq "sweep: unknown placeholder $tok is a usage error" 2 "$rc"
  expect_has "sweep: the error names the placeholder $tok" "unknown placeholder $tok" "$err"
  expect_eq "sweep: nothing sent for $tok" 0 "$(grep -c query_datasources "$STUB_LOG" || true)"
done
reset
echo '{"checks": [{"name": "only", "expr": "vector(1)"}], "replace_default_checks": true}' > "$WORK/only.json"
out=$(PR "${SW[@]}" --checks "$WORK/only.json")
expect_eq "sweep: replace_default_checks leaves only the file's checks" "C0 FSIZE FFREE FMIN FAGO" "$(body 1 | jq -r '[.queries[].refId] | join(" ")')"
reset
echo '{"disable_default_checks": ["up min", "reboots"]}' > "$WORK/dis.json"
out=$(PR "${SW[@]}" --checks "$WORK/dis.json")
if grep -q $'^up min\t\|^reboots\t' <<<"$out"; then bad "sweep: a disabled default check is still printed"; else ok "sweep: disable_default_checks drops the named checks"; fi
expect_has "sweep: the others stay" $'load1 max / cpu\t>2' "$out"
bad_file() { # name json
  printf '%s\n' "$2" > "$WORK/bad.json"; reset
  rc=0; err=$(PR "${SW[@]}" --checks "$WORK/bad.json" 2>&1 >/dev/null) || rc=$?
  expect_eq "sweep --checks: $1 is a usage error" 2 "$rc"
  expect_eq "sweep --checks: $1 sends nothing" 0 "$(grep -c query_datasources "$STUB_LOG" || true)"
}
bad_file "an unknown default name in disable_default_checks" '{"disable_default_checks": ["no such check"]}'
bad_file "an unknown key" '{"chekcs": []}'
bad_file "an unknown key in a check" '{"checks": [{"name": "a", "expr": "up", "flg": ">1"}]}'
bad_file "a bad flag" '{"checks": [{"name": "a", "expr": "up", "flag": "~3"}]}'
bad_file "a flag without a number" '{"checks": [{"name": "a", "expr": "up", "flag": ">"}]}'
bad_file "an empty name" '{"checks": [{"name": "", "expr": "up"}]}'
bad_file "a name of 61 characters" "{\"checks\": [{\"name\": \"$(printf 'n%.0s' $(seq 1 61))\", \"expr\": \"up\"}]}"
bad_file "a name with a control character" '{"checks": [{"name": "a\u001bb", "expr": "up"}]}'
bad_file "a name used twice" '{"checks": [{"name": "a", "expr": "up"}, {"name": "a", "expr": "up"}]}'
bad_file "a name of a default check" '{"checks": [{"name": "reboots", "expr": "up"}]}'
bad_file "no expr" '{"checks": [{"name": "a"}]}'
bad_file "31 checks" "$(jq -nc '{checks: [range(31) | {name: "c\(.)", expr: "up"}]}')"
bad_file "a file that is not an object" '[1]'
bad_file "replace_default_checks that is not a boolean" '{"replace_default_checks": "yes"}'
reset
echo '{"checks": '"$(jq -nc '[range(30) | {name: "c\(.)", expr: "up"}]')"'}' > "$WORK/thirty.json"
rc=0; PR "${SW[@]}" --checks "$WORK/thirty.json" >/dev/null 2>&1 || rc=$?
expect_eq "sweep --checks: 30 checks are accepted" 0 "$rc"
expect_eq "sweep --checks: 30 more checks stay one call" 1 "$(grep -c query_datasources "$STUB_LOG")"
sweep_usage() { # a usage error and nothing sent
  reset
  rc=0; PR sweep --ds prom-main "$@" >/dev/null 2>&1 || rc=$?
  expect_eq "sweep: usage error for $*" 2 "$rc"
  expect_eq "sweep: nothing sent for $*" 0 "$(grep -c query_datasources "$STUB_LOG" || true)"
}
sweep_usage --selector '' --last 12h
sweep_usage --selector "$SEL"
sweep_usage --selector "$SEL" --from 2026-11-20T03:00:00Z
sweep_usage --selector "$SEL" --last 12h --from 2026-11-20T03:00:00Z --to 2026-11-20T15:00:00Z
sweep_usage --selector "$SEL" --last 12h --trend 7x
sweep_usage --selector "$SEL" --last 12h --host-label 'a b'
sweep_usage --selector "$SEL" --from 2026-11-20T15:00:00Z --to 2026-11-20T03:00:00Z
sweep_usage --selector "$SEL" --last 12h --checks "$WORK/does-not-exist.json"
reset
rc=0; out=$(STUB_SWEEP_ERR=1 PR "${SW[@]}") || rc=$?
expect_eq "sweep: a failed refId exits 1" 1 "$rc"
expect_has "sweep: the failed check is named with its error" "# ERROR load1 max / cpu: parse error: unexpected end of input" "$out"
expect_has "sweep: the other checks still print" $'up min\t<1\t1' "$out"
reset
rc=0; out=$(STUB_FAIL_CALL=1 PR "${SW[@]}") || rc=$?
expect_eq "sweep: a failed request exits 1" 1 "$rc"
expect_has "sweep: a failed request is reported" "# ERROR request: HTTP 400 from the app" "$out"
expect_eq "sweep: a failed request is not retried" 1 "$(grep -c query_datasources "$STUB_LOG")"
for sub in compare sweep; do
  out=$(PR "$sub" --help)
  expect_has "grafana-prom $sub --help works" "usage: grafana-prom $sub" "$out"
done
expect_has "grafana-os --help works" "usage: grafana-os" "$(O --help)"
# --wide, --max-rows and --columns of grafana-os are the ones of grafana-query run
reset
out=$(O "${OSA[@]}" --hist 30m --wide --max-rows 1)
expect_has "grafana-os --wide: passed to grafana-query run" "## wide 2 series" "$out"
expect_has "grafana-os --max-rows: passed to grafana-query run" "more rows not shown" "$out"
rc=0; O "${OSA[@]}" --hist 30m --max-rows 0 >/dev/null 2>&1 || rc=$?
expect_eq "grafana-os --max-rows 0: the usage error of grafana-query run" 2 "$rc"
rc=0; O "${OSA[@]}" --hist 30m --account B >/dev/null || rc=$?
expect_has "grafana-os --account is passed through" "--account B" "$(cat "$STUB_LOG")"
out=$(G promql --help)
expect_has "promql --help names --wide and --max-rows" "--max-rows" "$out"

# --- everything printed passes through clean() -----------------------------------------------------------------
cat > "$WORK/esc.json" <<'JSON'
{"results": {"A": {"status": 200, "frames": [
 {"schema": {"fields": [{"name": "Time", "type": "time"}, {"name": "Value", "type": "number", "labels": {"h": "a\u001b[31mb"}, "config": {"displayNameFromDS": "x\u001b[0my"}}]},
  "data": {"values": [[1763643600000], [1]]}},
 {"schema": {"fields": [{"name": "Time", "type": "time"}, {"name": "Value", "type": "number", "labels": {"h": "c\u001b[1md"}}]},
  "data": {"values": [[1763643600000], [2]]}}]}}}
JSON
out=$(G frames "$WORK/esc.json" --wide)
if grep -q $'\033' <<<"$out"; then bad "wide / display names: an escape character reached the output"; else ok "wide / display names: control characters are replaced"; fi

# --- hygiene -------------------------------------------------------------------------------------------------------------------------
for s in "$SCRIPT" "$OSCRIPT" "$PSCRIPT"; do
  if grep -nE '\bcurl\b|\bwget\b|requests|urllib|http\.client|import socket|eval\(|exec\(|shell=True|os\.system|GRAFANA_|TOKEN|Authorization' "$s"; then
    bad "$(basename "$s") uses the network, a shell or credentials directly"
  else
    ok "$(basename "$s"): aurora-agent only, no shell, no credentials"
  fi
done
if [ -n "$(find "$TMPDIR/aurora-agent" -maxdepth 1 -name 'grafana-*' 2>/dev/null)" ]; then bad "temp files left behind"; else ok "temp files removed"; fi

if [ "$fails" -ne 0 ]; then echo "$fails check(s) failed" >&2; exit 1; fi
echo "grafana-query: all checks passed"
