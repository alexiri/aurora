#!/usr/bin/env bash
# Tests the four scripts that the built-in grafana package ships in its skills: `grafana-query`
# (grafana-query/scripts), `grafana-os` (grafana-opensearch/scripts), `grafana-prom`
# (grafana-prometheus/scripts; one script per skill because of the 32000-character skill limit) and
# `grafana-alert` (grafana-alerts/scripts), with no network: a stub
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
ASCRIPT="$SKILLS/grafana-alerts/scripts/grafana-alert"
FIX="$ROOT/scripts/plugins/fixtures/grafana"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
export TMPDIR="$WORK/tmp"
mkdir -p "$TMPDIR" "$WORK/bin" "$WORK/bodies"
export STUB_FIX="$FIX" STUB_LOG="$WORK/calls.log" STUB_BODIES="$WORK/bodies"
unset STUB_EMPTY STUB_FAIL_CALL STUB_NO_CP STUB_RULES STUB_PERSON_RULE STUB_RECORD_RULE STUB_MISMATCH_RULE STUB_NOEXPIRY_RULE STUB_GRAFANA12_RULE

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
  get_current_org) cp "$STUB_FIX/get_current_org.json" "$out" ;;
  list_contact_points) if [ -n "${STUB_NO_CP:-}" ]; then echo '[]' > "$out"; else cp "$STUB_FIX/list_contact_points.json" "$out"; fi ;;
  list_alert_rules) cp "$STUB_FIX/list_alert_rules${STUB_RULES:+-$STUB_RULES}.json" "$out" ;;
  get_alert_rule) if [ -n "${STUB_PERSON_RULE:-}" ]; then cp "$STUB_FIX/get_alert_rule_person.json" "$out"; elif [ -n "${STUB_RECORD_RULE:-}" ]; then cp "$STUB_FIX/get_alert_rule_record.json" "$out"; elif [ -n "${STUB_MISMATCH_RULE:-}" ]; then cp "$STUB_FIX/get_alert_rule_mismatch.json" "$out"; elif [ -n "${STUB_NOEXPIRY_RULE:-}" ]; then cp "$STUB_FIX/get_alert_rule_noexpiry.json" "$out"; elif [ -n "${STUB_GRAFANA12_RULE:-}" ]; then cp "$STUB_FIX/get_alert_rule_grafana12.json" "$out"; else cp "$STUB_FIX/get_alert_rule.json" "$out"; fi ;;
  list_alert_rule_states) cp "$STUB_FIX/list_alert_rule_states.json" "$out" ;;
  create_alert_rule|update_alert_rule)
    n=$(find "$STUB_BODIES" -type f | wc -l)
    cp "$body" "$STUB_BODIES/body-$((n + 1)).json"
    if [ "$op" = create_alert_rule ]; then echo '{"uid": "new-rule"}' > "$out"; else echo '{}' > "$out"; fi ;;
  delete_alert_rule) echo '{}' > "$out" ;;
  query_datasources)
    n=$(find "$STUB_BODIES" -type f | wc -l)
    cp "$body" "$STUB_BODIES/body-$((n + 1)).json"
    if grep -q '"expr": "bad' "$body" || [ "${STUB_FAIL_CALL:-}" = "$((n + 1))" ]; then cat "$STUB_FIX/query_error.txt" >&2; exit 1
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

for s in "$SCRIPT" "$OSCRIPT" "$PSCRIPT" "$ASCRIPT"; do
  if [ -x "$s" ]; then ok "$(basename "$s") is executable"; else bad "$(basename "$s") is not executable"; fi
  if python3 -I -m py_compile "$s"; then ok "py_compile $(basename "$s")"; else bad "py_compile $(basename "$s")"; fi
done
find "$SKILLS" -name __pycache__ -prune -exec rm -rf {} + 2>/dev/null || true
# The scripts each carry their own copy of the small helpers (a skill may hold at most 32000
# characters, so no shared module): the copies must stay identical.
if python3 -I - "$SCRIPT" "$OSCRIPT" "$PSCRIPT" "$ASCRIPT" <<'PY'
import ast, sys
files = dict(zip(("query", "os", "prom", "alert"), sys.argv[1:]))
want = {
    "query,os,prom": ["norm_time"],
    "query,os,prom,alert": ["clean", "die", "tmp_dir", "mktemp", "parse_step"],
    "query,prom": ["fit_step", "read_json", "send_body"],
    "query,prom,alert": ["agent"],
}
consts = {"query,os,prom,alert": ["CTRL", "UID_RE", "UNIT_S", "TIMEOUT"], "query,prom": ["POINTS_MAX"]}
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

out=$(PR compare --help)
expect_has "grafana-prom compare --help works" "usage: grafana-prom compare" "$out"
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

# --- grafana-alert: managed alert rules, guardrails and refusals -------------------------------------------------------
AL() { "$ASCRIPT" "$@"; }
refuse() {
  local want="$1"; shift
  local rc=0
  AL "$@" >"$WORK/out" 2>"$WORK/err" || rc=$?
  if [ "$rc" -eq 0 ]; then
    bad "grafana-alert $* was expected to refuse with '$want'"
  elif grep -qF -- "$want" "$WORK/err"; then
    ok "grafana-alert refuses: $want"
  else
    bad "grafana-alert refused ($rc) without '$want'"; cat "$WORK/err" >&2
  fi
}
EXP=$(date -u -d "+14 days" +%F)

# create builds the exact body: the three-step model, the guard labels and the expiry
reset
if AL create --managed-by night --folder night-folder --group agent-night --title "p95 over 2s" \
  --ds prom-main --expr 'max(rate(http_request_duration_seconds_count{job="api"}[5m]))' \
  --op gt --threshold 2 --for 5m --receiver oncall --expires 14d --service api >"$WORK/out" \
  && python3 -I - "$WORK/bodies/body-1.json" "$EXP" <<'PY'
import json, sys
body, exp = json.load(open(sys.argv[1])), sys.argv[2]
def step(ref, dsu, model):
    return {"refId": ref, "datasourceUid": dsu, "queryType": "",
            "relativeTimeRange": {"from": 300, "to": 0}, "model": model}
def cond(ref, ty, params):
    return {"evaluator": {"params": params, "type": ty}, "operator": {"type": "and"},
            "query": {"params": [ref]}, "reducer": {"params": [], "type": "last"}, "type": "query"}
x = {"type": "__expr__", "uid": "__expr__"}
expr = 'max(rate(http_request_duration_seconds_count{job="api"}[5m]))'
want = {"orgID": 1, "title": "p95 over 2s", "ruleGroup": "agent-night", "folderUID": "night-folder",
        "condition": "C", "for": "300s", "noDataState": "NoData", "execErrState": "Error",
        "isPaused": False, "labels": {"managed_by": "night", "expires": exp, "service": "api"},
        "annotations": {"summary": "p95 over 2s", "expires": exp},
        "notification_settings": {"receiver": "oncall"},
        "data": [step("A", "prom-main", {"editorMode": "code", "expr": expr, "instant": True,
                                         "legendFormat": "__auto", "refId": "A"}),
                 step("B", "__expr__", {"conditions": [cond("B", "gt", [])], "datasource": x,
                                        "expression": "A", "reducer": "last", "refId": "B",
                                        "type": "reduce"}),
                 step("C", "__expr__", {"conditions": [cond("C", "gt", [2.0])], "datasource": x,
                                        "expression": "B", "refId": "C", "type": "threshold"})]}
if body == want:
    sys.exit(0)
for k in want:
    if body.get(k) != want[k]:
        print(f"create body[{k}]:\n  want {want[k]!r}\n  got  {body.get(k)!r}", file=sys.stderr)
sys.exit(1)
PY
then ok "create builds the exact body (three-step model, guard labels, expires, receiver)"; else bad "create body differs"; fi

refuse "create needs --expires" create --managed-by night --folder night-folder --group agent-night \
  --title t --ds prom-main --expr 'x[5m]' --op gt --threshold 1 --for 5m --receiver oncall
refuse "--expires is 1d to 30d" create --managed-by night --folder night-folder --group agent-night \
  --title t --ds prom-main --expr 'x[5m]' --op gt --threshold 1 --for 5m --receiver oncall --expires 31d
refuse "a range of at least 5 minutes" create --managed-by night --folder night-folder \
  --group agent-night --title t --ds prom-main --expr 'cpu_seconds > 1' --op gt --threshold 1 \
  --for 5m --receiver oncall --expires 14d
STUB_NO_CP=1 refuse "no contact point named 'nope'" create --managed-by night --folder night-folder \
  --group agent-night --title t --ds prom-main --expr 'x[5m]' --op gt --threshold 1 --for 5m \
  --receiver nope --expires 14d
refuse "is a loki data source" create --managed-by night --folder night-folder --group agent-night \
  --title t --ds loki-main --expr 'x[5m]' --op gt --threshold 1 --for 5m --receiver oncall --expires 14d
STUB_RULES=cap refuse "already manages 10 rules" create --managed-by night --folder night-folder \
  --group agent-night --title t --ds prom-main --expr 'x[5m]' --op gt --threshold 1 --for 5m \
  --receiver oncall --expires 14d
refuse "evaluates every 10s" create --managed-by night --folder night-folder --group fast \
  --title t --ds prom-main --expr 'x[5m]' --op gt --threshold 1 --for 5m --receiver oncall --expires 14d

# update and delete refuse anything the agent does not manage, or outside its folder
STUB_PERSON_RULE=1 refuse "not managed by 'night'" update cpu-watch --managed-by night \
  --folder night-folder
refuse "is in folder night-folder, not other-folder" update tripwire-p95 --managed-by night \
  --folder other-folder
refuse "pass --renew with --reason" update tripwire-p95 --managed-by night --folder night-folder \
  --expires 14d
STUB_PERSON_RULE=1 refuse "not managed by 'night'" delete cpu-watch --managed-by night \
  --folder night-folder

# update refuses a rule that left the shape the script builds (recording rule, extra label,
# extra receiver setting): none of it is sent back
STUB_RECORD_RULE=1 refuse "a recording rule" update tripwire-p95 --managed-by night \
  --folder night-folder

# the expiry is read from the `expires` label only; the annotation is a copy the script writes the
# same, so a rule whose two disagree was changed outside the script and is refused
STUB_MISMATCH_RULE=1 refuse "the expires label (2026-11-20) and the expires annotation (2027-01-01) disagree" \
  update tripwire-p95 --managed-by night --folder night-folder
STUB_RULES=mismatch refuse "the expires label (2026-11-20) and the expires annotation (2027-01-01) disagree" \
  expire --managed-by night --folder night-folder

# expire is folder-scoped and says so; a rule with no readable expiry needs --renew --reason to
# get one (setting an expiry may be extending how long the rule lives after all)
refuse "expire needs --folder" expire --managed-by night
STUB_NOEXPIRY_RULE=1 refuse "the rule has no readable expiry" update tripwire-p95 --managed-by night \
  --folder night-folder --expires 14d
STUB_NOEXPIRY_RULE=1 refuse "the rule has no readable expiry" update tripwire-p95 --managed-by night \
  --folder night-folder --expires 14d --renew

# update sends the whole changed rule; the read-only fields are stripped
reset
if AL update tripwire-p95 --managed-by night --folder night-folder --title "p95 over 3s" --for 10m \
  --receiver nightmanager --expires 7d --renew --reason "keep the tripwire a week longer" >"$WORK/out" \
  && python3 -I - "$WORK/bodies/body-1.json" "$(date -u -d "+7 days" +%F)" <<'PY'
import json, sys
body, exp = json.load(open(sys.argv[1])), sys.argv[2]
problems = []
if body.get("title") != "p95 over 3s":
    problems.append(f"title {body.get('title')!r}")
if body.get("for") != "600s":
    problems.append(f"for {body.get('for')!r}")
if (body.get("notification_settings") or {}).get("receiver") != "nightmanager":
    problems.append(f"receiver {body.get('notification_settings')!r}")
if (body.get("labels") or {}).get("expires") != exp:
    problems.append(f"labels.expires {body.get('labels')!r}")
if (body.get("annotations") or {}).get("expires") != exp:
    problems.append(f"annotations.expires {body.get('annotations')!r}")
for gone in ("id", "updated", "updatedBy", "created", "createdBy", "version", "provenance", "metadata"):
    if gone in body:
        problems.append(f"read-only field {gone} sent back")
if len(problems) == 0:
    sys.exit(0)
print("; ".join(problems), file=sys.stderr)
sys.exit(1)
PY
then ok "update sends the changed rule (new title, for, receiver, expiry), read-only fields stripped"; else bad "update body differs"; fi

# a rule read back from a newer Grafana carries fields the script never writes (keep_firing_for,
# missingSeriesEvalsToResolve); update still accepts it and sends them back unchanged
reset
if STUB_GRAFANA12_RULE=1 AL update tripwire-p95 --managed-by night --folder night-folder --title "p95 over 3s" --for 10m \
  --receiver nightmanager --expires 7d --renew --reason "keep the tripwire a week longer" >"$WORK/out" \
  && python3 -I - "$WORK/bodies/body-1.json" <<'PY'
import json, sys
body = json.load(open(sys.argv[1]))
problems = []
if body.get("keep_firing_for") != "0s":
    problems.append(f"keep_firing_for {body.get('keep_firing_for')!r}")
if body.get("missingSeriesEvalsToResolve") != 2:
    problems.append(f"missingSeriesEvalsToResolve {body.get('missingSeriesEvalsToResolve')!r}")
for gone in ("id", "updated", "provenance"):
    if gone in body:
        problems.append(f"read-only field {gone} sent back")
if len(problems) == 0:
    sys.exit(0)
print("; ".join(problems), file=sys.stderr)
sys.exit(1)
PY
then ok "update accepts a rule read back with keep_firing_for and missingSeriesEvalsToResolve, read-only fields stripped"; else bad "update of the newer Grafana rule differs"; fi

# expire deletes only the expired managed ones, each delete still held for approval
reset
out=$(AL expire --managed-by night --folder night-folder)
if grep -qF "1 deleted" <<<"$out" && [ "$(grep -c 'delete_alert_rule' "$STUB_LOG")" = 1 ] \
  && grep -qF 'delete_alert_rule uid=tripwire-p95' "$STUB_LOG" \
  && ! grep -qF 'uid=tripwire-5xx' "$STUB_LOG" && ! grep -qF 'uid=cpu-watch' "$STUB_LOG"; then
  ok "expire deletes only the expired managed rule (tripwire-p95)"
else
  bad "expire did not delete exactly the expired managed rule"; echo "$out" >&2; cat "$STUB_LOG" >&2
fi

# list shows the agent's managed rules, not other people's
out=$(AL list --managed-by night)
if grep -qF 'tripwire-p95' <<<"$out" && grep -qF 'tripwire-5xx' <<<"$out" \
  && ! grep -qF 'cpu-watch' <<<"$out"; then
  ok "list shows the managed rules and not other people's"
else
  bad "list output wrong"; echo "$out" >&2
fi

# --- size budgets (D-484): the app must not grow back silently ---------------------------------------------------------
PKG="$ROOT/plugins/builtin/grafana"
total=$(find "$SKILLS" -type f -exec cat {} + | wc -c)
bodies=$(cat "$SKILLS"/*/SKILL.md | wc -c)
echo "sizes: all skill files $total (budget 75000), SKILL.md bodies $bodies (budget 20000)"
for f in "$SKILLS"/*/SKILL.md "$SCRIPT" "$OSCRIPT" "$PSCRIPT" "$ASCRIPT"; do echo "sizes: $(wc -c < "$f") ${f#"$SKILLS"/}"; done
if [ "$total" -le 75000 ]; then ok "all skill files together are at most 75000 characters ($total)"; else bad "all skill files are $total characters, over 75000"; fi
if [ "$bodies" -le 20000 ]; then ok "SKILL.md bodies together are at most 20000 characters ($bodies)"; else bad "SKILL.md bodies are $bodies characters, over 20000"; fi
expect_eq "exactly four skills" "grafana-alerts grafana-opensearch grafana-prometheus grafana-query" "$(find "$SKILLS" -mindepth 1 -maxdepth 1 -type d -printf '%f\n' | sort | paste -sd' ')"
expect_eq "only grafana-query's description says to read it first" "grafana-query" \
  "$(for f in "$SKILLS"/*/SKILL.md; do grep -m1 '^description:' "$f" | grep -qi 'first' && basename "$(dirname "$f")"; done | paste -sd' ')"
if grep -rnIE 'grafana-api-notes|investigate-a-spike|sweep' "$PKG" >&2; then bad "a removed skill or subcommand is still mentioned in the package"; else ok "no mention of grafana-api-notes, investigate-a-spike or sweep in the package"; fi

# --- hygiene -------------------------------------------------------------------------------------------------------------------------
for s in "$SCRIPT" "$OSCRIPT" "$PSCRIPT" "$ASCRIPT"; do
  if grep -nE '\bcurl\b|\bwget\b|requests|urllib|http\.client|import socket|eval\(|exec\(|shell=True|os\.system|GRAFANA_|TOKEN|Authorization' "$s"; then
    bad "$(basename "$s") uses the network, a shell or credentials directly"
  else
    ok "$(basename "$s"): aurora-agent only, no shell, no credentials"
  fi
done
if [ -n "$(find "$TMPDIR/aurora-agent" -maxdepth 1 -name 'grafana-*' 2>/dev/null)" ]; then bad "temp files left behind"; else ok "temp files removed"; fi

if [ "$fails" -ne 0 ]; then echo "$fails check(s) failed" >&2; exit 1; fi
echo "grafana-query: all checks passed"
