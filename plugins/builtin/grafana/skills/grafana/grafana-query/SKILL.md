---
name: grafana-query
description: "Run Prometheus, Mimir, Loki, OpenSearch, Elasticsearch or SQL queries through Grafana and read the results; read before the first query"
---

# Query data sources through Grafana

Use for any request that needs numbers, log lines or rows from a data source behind Grafana.
`$G` is the Grafana address from `app.md` (it may end with a path such as `/grafana`); Aurora adds
the token. The helper script does the request building and the frame reading:

```sh
S="$SKILLS_DIR/grafana-query/scripts/grafana-query"
"$S" promql --ds <uid> --from now-6h --to now --step 5m 'sum(rate(http_requests_total[5m]))'
```

`--help` on each subcommand. Several Grafana accounts: `--account <letter>`. Output options of
`promql`, `logql`, `run` and `frames`: `--wide` (one table per refId, a `Time` column and one column
per series) and `--max-rows N` (1 to 5000, default 200 per frame). `--instant` needs only `--to`.
OpenSearch queries from flags: `grafana-opensearch` (`grafana-os`). Window comparison and health
sweep: `grafana-prometheus` (`grafana-prom compare`, `sweep`). Checked against
Grafana v13.3.1 (`pkg/api/ds_query.go`, `pkg/services/query/query.go`) and, for the Prometheus and
Loki query models, Grafana v12.3.0, the last release that has them in its source tree.

## Why `ds/query` comes first

`query_datasources` is `POST /api/ds/query`, the route Grafana's own UI uses for every data source.
It is a read here (it runs as the service account under the Viewer role). The data source **proxy**
tools (`prometheus_*`, `loki_*`, `GET /api/datasources/proxy/uid/<uid>/…`) forward to the backend's
own address, and a backend that redirects (http to https, or a path) answers with a `3xx` that
`aurora-agent` never follows (the new host is not this connection's): on such an instance (seen once,
HTTP 308) the proxy tools never work, while `ds/query` does. Use `ds/query` first; the proxy only
adds series, labels, label values and rules, and those paths return 3xx there too. Use these instead:

- Metric names: an instant query `count by (__name__) ({<selector>, __name__=~"<prefix>.*"})`, such
  as `"$S" promql --ds <uid> --from now-1h --to now --instant 'count by (__name__) ({job="x", __name__=~"http_.*"})'`.
- Label values: `count by (<label>) (<metric>{<selector>})`, with `--instant` the same way.
- Label names and expressions in use: `grafana-query panels <dashboard uid>`.
- Rules: Grafana-managed ones with `list_alert_rule_states` / `list_alert_rules`. The data
  source's own rules (`prometheus_rules`) cannot be reached; say so as a data gap.

Only read statements. Data sources should use read-only database users; a SQL or InfluxDB data
source whose user can write could be changed by a query (D-462). Never send `INSERT`, `UPDATE`,
`DELETE`, `DROP`, `SELECT … INTO` or similar.

## Find the data source UID

1. Run `"$S" datasources` first; it lists `uid  type  name` from `list_datasources`. Whether Grafana
   shows this list depends on the token's data source permissions: some instances list every
   source to a Viewer or Editor token, while with Grafana's defaults the list is empty or holds only
   the built-in `grafana` source. If it is empty, use step 2.
2. Otherwise read a dashboard that uses the data source: `"$S" panels <dashboard uid>` prints every
   panel (rows expanded) with its data source (a `${datasource}` variable is resolved through
   `templating.list[].current`) and each target's `expr`, `query`, `rawSql`, `metrics`, `bucketAggs`
   and `timeField`. Find the dashboard with `search`.
3. `check_datasource_health uid=<uid>` confirms Grafana reaches the backend.

**Panels are the field-name documentation.** Read a panel's `targets[]` (`expr`,
`metrics[].field`, `bucketAggs[].field`, `timeField`) before guessing a field or a label name.

## The request

```json
{"from": "now-6h", "to": "now",
 "queries": [{"refId": "A", "datasource": {"uid": "<uid>", "type": "prometheus"}, "expr": "up", "range": true,
              "intervalMs": 300000, "maxDataPoints": 73}]}
```

Send it with `aurora-agent call grafana query_datasources --body @body.json --out /tmp/aurora-agent/q.json`
(an absolute `--out`), or `"$S" run body.json`, which also prints the frames.

- `from` and `to`: epoch milliseconds (a string), or date math such as `now-6h`
  (`ds/query` uses the plugin SDK's parser: epoch ms, a duration, or Grafana date math). The script
  also accepts RFC 3339 and converts it.
- Every query needs a unique `refId` and `datasource.uid` (the `type` is informative: Grafana picks the
  plugin by the UID). Queries for several data sources can share one request; Grafana runs them
  concurrently.
- Defaults when omitted: `maxDataPoints` 100 and `intervalMs` 1000 (`query.go` 453-454), so set
  both for anything but a tiny window. The points per series are about `(to - from) / interval`.
  **Prometheus refuses more than 11,000 points per series**; the script refuses first and names a
  step that fits.
- Batch refIds that share a window in one request.
- **Times for evidence.** Convert the person's window to absolute UTC once (the sandbox runs in UTC,
  the person's time zone may not) and use RFC 3339 times (`2026-11-20T13:00:00Z`) in every query
  that goes into a report, never `now-12h`: it could not be re-run with the same answer.

### Prometheus and Mimir

Range: `{"refId":"A","datasource":{"uid":"<uid>","type":"prometheus"},"expr":"<PromQL>","range":true,"intervalMs":<step ms>,"maxDataPoints":<n>}`.
Instant: `"instant": true` instead of `range` (one point per series at `to`). Neither flag means a range
query. `"interval": "5m"` sets the minimum step, `"format": "table"` a table frame, `"legendFormat"`
the series name. Script: `promql --ds <uid> --from … --to … [--step 5m | --instant] <expr>…`.

### Loki

`{"refId":"A","datasource":{"uid":"<uid>","type":"loki"},"expr":"<LogQL>","queryType":"range","maxLines":100}`.
`queryType` is `range` (log lines **and** metric queries over a range; `step` such as `"1m"` for
metric queries) or `instant` (a metric query at `to`). Script: `logql … [--limit N]`. Keep
`maxLines` small.

### OpenSearch and Elasticsearch

Use `grafana-os` and see `grafana-opensearch` (the flags, body format, aggregations, raw documents,
quirks).

### SQL

`{"refId":"A","datasource":{"uid":"<uid>","type":"postgres"},"rawSql":"SELECT … LIMIT 100","format":"table"}`.
Read statements only, with a `LIMIT`. Time series: `format: "time_series"` with a `time` column and
use `$__timeFilter(col)` in the statement.

## Reading the answer

`{"results": {"<refId>": {"status": 200, "frames": [ … ]}}}`. A frame is columns, not rows:
`schema.fields[i]` (`name`, `type`, `labels`) describes column `data.values[i]`. For time series each
series is one frame with a `Time` column (epoch milliseconds) and a `Value` column whose `labels`
name the series (`{job, instance, …}`). An instant query is one point per frame. A table or
long-format frame has one column per field (several value columns, one row per `terms` bucket). The
script prints, for each refId, `# <refId> <status>` (and the error), then per frame a header with
its labels or field names and tab-separated rows with times in RFC 3339 UTC; `"$S" frames q.json`
prints a saved answer.

**Column names.** A field is printed as `name{labels}`. A data source may give it a display name
(`config.displayNameFromDS`; OpenSearch: `<term> Count`, `<term> Sum <field>`): for a field called
`Value` that is the column name, otherwise it follows as `name{labels} [display name]`.

**Never quote a count from cut output.** Rows over `--max-rows` are cut with
`... N more rows not shown (M in all)`: use M. A `terms` answer with exactly `size` buckets is only a
lower bound: write "at least N", or raise `size`.

For documents (`raw_data`), `--columns a,b,c` on `run` or `frames` prints only those fields (plus
the time field). Use it: access-log documents have dozens of fields.

### Errors

- Each refId has its own `status` and `error`. If any refId fails, Grafana answers HTTP 400 for
  the whole request (`ds_query.go` `toJsonStreamingResponse`). The script handles this by retrying
  each refId alone, so the output shows every refId that worked and the error of the one that
  failed. By hand with `aurora-agent`, only the first lines of the body are printed and no file is
  saved, so send a failing batch again one refId at a time.
- `errorSource: "plugin"` marks an error the data source plugin raised (a bad query or field, a
  failing backend); `"downstream"` means the backend answered with an error. Both come with a
  message: read it, fix the query.
- `403`/`404` for the request: no permission for that data source, or no such UID. A `3xx` from the
  proxy tools: use `query_datasources`.
- Keep ranges and steps small; ask for fewer series (`topk`, `by (…)`) before widening the window.
