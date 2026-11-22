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

`--help` on each subcommand. Several Grafana accounts: `--account <letter>`. Checked against
Grafana v13.3.1 (`pkg/api/ds_query.go`, `pkg/services/query/query.go`) and, for the Prometheus and
Loki query models, Grafana v12.3.0, the last release that has them in its source tree.

## Why `ds/query` comes first

`query_datasources` is `POST /api/ds/query`, the route Grafana's own UI uses for every data source.
It is a read here (it runs as the service account under the Viewer role). The data source **proxy**
tools (`prometheus_*`, `loki_*`, `GET /api/datasources/proxy/uid/<uid>/…`) forward to the backend's
own address, and a backend that redirects (http to https, or a path) answers Grafana's proxy with a
`3xx` that Grafana passes through unchanged. `aurora-agent` never follows redirects (the new host is
not this connection's), so on such an instance the proxy tools never work, while `ds/query` does.
Seen on one instance (HTTP 308 with a `Location` on the backend host). Use `ds/query` first; use the
proxy only for the paths it offers that `ds/query` does not (series, labels, label values, rules).

Only read statements. Data sources should use read-only database users; a SQL or InfluxDB data
source whose user can write could be changed by a query (D-462). Never send `INSERT`, `UPDATE`,
`DELETE`, `DROP`, `SELECT … INTO` or similar.

## Find the data source UID

1. `"$S" datasources` lists `uid  type  name` from `list_datasources`. Grafana shows that list only
   to tokens allowed to read data source settings (Admin by default): with a Viewer or Editor token
   it is empty or has only the built-in `grafana` source.
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

See `grafana-opensearch` (body format, aggregations, raw documents, quirks).

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

### Errors

- Each refId has its own `status` and `error`. **When any refId fails, Grafana answers HTTP 400 for
  the whole request** (`ds_query.go` `toJsonStreamingResponse`), and `aurora-agent` then prints only
  the first lines of the body and saves no file, so the data of the refIds that worked is lost. The
  script retries each refId alone in that case. By hand, send a failing batch again one refId at a
  time.
- `errorSource: "plugin"` marks an error the data source plugin raised (a bad query or field, a
  failing backend); `"downstream"` means the backend answered with an error. Both come with a
  message: read it, fix the query.
- `403`/`404` for the request: no permission for that data source, or no such UID. A `3xx` from the
  proxy tools: use `query_datasources`.
- Keep ranges and steps small; ask for fewer series (`topk`, `by (…)`) before widening the window.
