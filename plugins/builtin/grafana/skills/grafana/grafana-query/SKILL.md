---
name: grafana-query
description: "Search Grafana dashboards, alerts and annotations and query Prometheus, Mimir, Loki, OpenSearch, Elasticsearch or SQL data sources through it; read before the first Grafana request"
---

# Grafana: dashboards, alerts and data source queries

`$G` is the Grafana address from `app.md` (it may end with a path such as `/grafana`). Aurora adds the
token: send none, no `-k`, never send this connection's requests to another host. The token is a service
account's: one organization, its role (Viewer or Editor), its folder and data source permissions.
`aurora-agent search grafana <words>` finds operations; the script builds queries and reads frames:

```sh
S="$SKILLS_DIR/grafana-query/scripts/grafana-query"
"$S" promql --ds <uid> --from now-6h --to now --step 5m 'sum(rate(http_requests_total[5m]))'
```

Subcommands `datasources`, `panels`, `promql`, `logql`, `run`, `frames` (`--help` on each); several
accounts: `--account <letter>`. `promql`, `logql`, `run`, `frames` take `--wide` (one table per refId, one
column per series) and `--max-rows N` (1 to 5000, default 200 per frame); `--instant` needs only `--to`.
OpenSearch from flags: `grafana-opensearch`. Window comparison: `grafana-prometheus`.

## Query through `ds/query`, not the proxy

`query_datasources` is `POST /api/ds/query`, the route Grafana's UI uses; it is a read here. The proxy tools
(`prometheus_*`, `loki_*`) forward to the backend's own address; if that redirects (http to https) they
answer `3xx`, which `aurora-agent` never follows (seen once, HTTP 308). On `3xx` use `ds/query`; instead of
the proxy:

- Metric names: `"$S" promql --ds <uid> --from now-1h --to now --instant 'count by (__name__) ({job="x", __name__=~"http_.*"})'`;
  label values: `count by (<label>) (<metric>{<selector>})`, also `--instant`.
- Label names and expressions in use: `"$S" panels <dashboard uid>`.
- Rules: `list_alert_rule_states` / `list_alert_rules` (Grafana-managed); the data source's own
  (`prometheus_rules`) is unreachable: report a data gap.

Read statements only: a SQL or InfluxDB data source whose user can write could be changed by a
query. Never send `INSERT`, `UPDATE`, `DELETE`, `DROP`, `SELECT … INTO`.

## Dashboards and the data source UID

- Find with `search query=<words> type=dash-db` (also `tag`, `folderUIDs`, `limit`; ask when two titles look
  alike); a link `…/d/<uid>/<slug>` holds the UID. Also `list_folders`, `get_folder`, `list_dashboard_tags`,
  `list_dashboard_versions` (needs edit permission).
- `get_dashboard uid=<uid>` returns `{dashboard, meta}`: `panels[]` (rows nest `panels[]`) with `datasource`
  and `targets[]`, `templating.list[]` (variables, `current`), `meta.folderUid`. `"$S" panels <uid>` prints
  each panel with its data source (a `${datasource}` variable resolved) and each target's `expr`, `query`,
  `rawSql`, `metrics`, `bucketAggs`, `timeField`: **panels document the field and label names**, read them
  before guessing.
- UID: `"$S" datasources` lists `uid type name`, but under Grafana's defaults it is empty or holds only the
  built-in `grafana` source (some instances list all) though queries work. Then take it from a panel
  (a `${…}` is a variable: `templating.list[].current.value`) or ask the person.
  `check_datasource_health uid=<uid>` confirms Grafana reaches the backend. `get_datasource` shows
  settings, never secrets. Aurora does not offer `/api/frontend/settings` (it carries direct-access
  passwords).
- Link a dashboard at a window: `$G/d/<uid>?from=<ms>&to=<ms>` (`&viewPanel=<panel id>`).

## Alerts and annotations

- `list_alert_rule_states`: rules by folder and group with `state` (`inactive`, `pending`, `firing`) and
  `alerts[]` (`dashboard_uid`, `folder_uid`, `state=firing`). `list_alert_rules`, `get_alert_rule`: definitions.
- `list_alerts`: instances with `labels`, `startsAt`, `status.state` (`active`, `suppressed`);
  `list_silences`: `active`, `pending`, `expired`.
- `list_annotations from=<ms> to=<ms>` (also `dashboardUID`, `tags`, `type=annotation|alert`, `limit`):
  deployments and incidents; `type=alert` shows past state changes.

## Writing: held for approval, only when the person asks

`create_annotation` (`text`, `tags`, `time`, `timeEnd` in ms; add `dashboardUID` and `panelId` to pin it,
else it is organization-wide), `update_dashboard`, `create_silence` (`matchers` of `name`, `value`,
`isRegex`, `isEqual`; `startsAt`, `endsAt`, `comment`; an `id` changes that silence), `delete_silence
id=<id>`: `aurora-agent call grafana <operation> --body @file.json` or single fields
(`text="Deployed v2" tags:='["deploy"]'`); `aurora-agent describe grafana <operation>` lists them. To save a
dashboard, read it first and change that: keep `uid`, `version` and `meta.folderUid` (a missing `folderUid`
moves it to the top level) and `overwrite: false`; on `412` read again.

## The request

```json
{"from": "now-6h", "to": "now",
 "queries": [{"refId": "A", "datasource": {"uid": "<uid>", "type": "prometheus"}, "expr": "up", "range": true,
              "intervalMs": 300000, "maxDataPoints": 73}]}
```

Send it with `aurora-agent call grafana query_datasources --body @body.json --out /tmp/aurora-agent/q.json`
(absolute `--out`), or `"$S" run body.json`, which also prints the frames.

- `from`/`to`: epoch ms or date math (`now-6h`); the script also takes RFC 3339.
- Each query needs a unique `refId` and `datasource.uid`; batch refIds that share a window.
- Defaults `maxDataPoints` 100, `intervalMs` 1000: set both. Points per series ≈ `(to - from) / interval`;
  **Prometheus refuses more than 11,000**, the script names a step that fits.
- **Evidence**: convert the person's window to absolute UTC once and use RFC 3339 times in every query that
  goes into a report, never `now-12h` (it cannot be re-run).
- **Prometheus/Mimir**: range by default, `"instant": true` for one point at `to`; `interval` (minimum
  step), `format: "table"`, `legendFormat`. `promql --ds <uid> --from … --to … [--step 5m | --instant] <expr>…`.
- **Loki**: `"expr":"<LogQL>","queryType":"range","maxLines":100` (`range` gives lines and metric queries,
  with `step`; `instant` a metric query at `to`). `logql … [--limit N]`.
- **SQL**: `"rawSql":"SELECT … LIMIT 100","format":"table"`; always a `LIMIT`; `format: "time_series"` needs
  a `time` column and `$__timeFilter(col)`.
- **OpenSearch, Elasticsearch**: `grafana-opensearch`.

## Reading the answer

`{"results": {"<refId>": {"status": 200, "frames": […]}}}`. A frame is columns: `schema.fields[i]` (`name`,
`type`, `labels`) describes `data.values[i]`; a time series is one frame per series (`Time` in epoch ms, `Value`
with `labels`). The script prints `# <refId> <status>` (and the error), then per frame a header and
tab-separated rows, times in RFC 3339 UTC; `"$S" frames q.json` prints a saved answer. A field is
`name{labels}`, then `[display name]` when the data source gives one. `--columns a,b,c` (`run`, `frames`)
prints only those fields of documents.

**Never quote a count from cut output.** Rows over `--max-rows` end in `... N more rows not shown (M in all)`:
use M. A `terms` answer with exactly `size` buckets is a lower bound: "at least N", or raise `size`.

## Errors

- Each refId has its own `status` and `error`; if any fails Grafana answers HTTP 400 for the whole request.
  The script retries each refId alone and shows those that worked; by hand, resend one refId at a time.
  `errorSource` `plugin` (bad query or field) or `downstream` (backend error): read the message, fix the query.
- `400` bad parameters (`UID is invalid`: only letters, digits, `-`, `_`); `401` token invalid or expired,
  the person reconnects; `403` role or folder or data source permissions, say which request; `404` no such
  dashboard, folder, rule or data source, or not visible; `412` dashboard version mismatch; `422`/`502` the
  backend refused, its message is in the body.
- Keep ranges and steps small; fewer series (`topk`, `by (…)`) before a wider window.
