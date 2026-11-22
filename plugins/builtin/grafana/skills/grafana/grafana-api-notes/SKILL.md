---
name: grafana-api-notes
description: "How to search Grafana dashboards, find data source UIDs, query data sources, alerting and annotations, and its errors; read before the first request"
---

# Grafana API notes

Search, dashboards, data source UIDs, the proxy paths, alerting, annotations, errors. `$G` is the
Grafana address shown with the account (it may end with a path such as `/grafana`). Aurora adds the
token: send none, no `-k`, and never send this connection's requests to another host. The token is a
service account's; it sees one Grafana organization, with that account's role (Viewer or Editor)
and folder and data source permissions.

## Common requests

| Task | Request |
|---|---|
| Who am I, which organization | `GET $G/api/user`, `GET $G/api/org` |
| Find dashboards | `GET $G/api/search?query=<words>&type=dash-db` (`tag=`, `folderUIDs=`, `limit=`, `page=`) |
| Folders | `GET $G/api/folders` (`parentUid=` for subfolders), `GET $G/api/folders/<uid>` |
| One dashboard (panels, queries) | `GET $G/api/dashboards/uid/<uid>` |
| A dashboard's versions | `GET $G/api/dashboards/uid/<uid>/versions` (needs edit permission) |
| Dashboard tags | `GET $G/api/dashboards/tags` |
| Query any data source (Prometheus, Mimir, Loki, OpenSearch, SQL) | `POST $G/api/ds/query` = `aurora-agent call grafana query_datasources --body @body.json`; the `grafana-query` skill's script builds it |
| Data source UIDs | `GET $G/api/datasources` (`list_datasources`, usually empty for Viewer and Editor), else a dashboard's panels (`grafana-query panels <uid>`) |
| Prometheus/Mimir through the proxy (fallback) | `GET $G/api/datasources/proxy/uid/<ds>/api/v1/query_range?query=…&start=…&end=…&step=60s` |
| Loki through the proxy (fallback) | `GET $G/api/datasources/proxy/uid/<ds>/loki/api/v1/query_range?query=…&limit=100` |
| Firing alerts | `GET $G/api/alertmanager/grafana/api/v2/alerts` |
| Rule states | `GET $G/api/prometheus/grafana/api/v1/rules` |
| Annotations | `GET $G/api/annotations?from=<ms>&to=<ms>` |

## Dashboards

A dashboard URL is `$G/d/<uid>/<slug>`. `GET /api/dashboards/uid/<uid>` returns `{dashboard, meta}`:
`dashboard.panels[]` (rows nest panels in `panels[].panels[]`), each with `title`, `datasource`
(`{type, uid}`) and `targets[]` (`expr` for Prometheus and Loki, `rawSql` for SQL, `query` for
others); `dashboard.templating.list[]` holds the variables and their `current` values;
`meta.folderUid` is the folder.

Saving (`POST /api/dashboards/db`, held for approval) takes
`{"dashboard": {…, "uid": "<uid>", "version": <n>}, "folderUid": "<folder uid>", "message": "…", "overwrite": false}`.
Read the current dashboard first and change it, keep its `uid`, `version` and `meta.folderUid` (a
missing `folderUid` moves the dashboard to the top level), and keep `overwrite: false`: a `412`
`version-mismatch` means someone saved in between, so read again instead of overwriting.

## Data source UIDs

Queries name a data source by UID. Take it from the panel you are reading (`datasource.uid`; a
`${…}` value is a dashboard variable, see `templating.list[].current.value`), or list them:
`GET /api/datasources` shows only the data sources the token may **read** settings of. Grafana gives
that permission to Admin by default, so with a Viewer or Editor token the list is usually empty or
holds only the built-in `grafana` source, even though queries work. Grafana's browser settings
(`/api/frontend/settings`) do list every data source a user can query, but they also carry the
passwords of direct-access data sources, so Aurora does not offer them. If you have no UID, run
`grafana-query panels <dashboard uid>` on a dashboard that uses the data source, or ask the person
for one. `GET /api/datasources/uid/<uid>` returns its
settings: `secureJsonFields` only says which secrets are set, never their values.

## The data source proxy

`$G/api/datasources/proxy/uid/<ds uid>/<backend path>` forwards to the data source (Grafana adds the
backend's credentials and tenant headers). **Use `query_datasources` for queries** (below): the proxy
forwards to the data source's own address, and when that backend redirects, Grafana passes the
`3xx` through and `aurora-agent` (which never follows redirects) cannot use it. A `3xx` from a proxy
tool means: switch to `query_datasources`. Only these backend paths, with `GET`:

- Prometheus and Mimir: `api/v1/query`, `api/v1/query_range`, `api/v1/series`, `api/v1/labels`,
  `api/v1/label/<name>/values`, `api/v1/metadata`, `api/v1/rules`, `api/v1/alerts`,
  `api/v1/query_exemplars`.
- Loki: `loki/api/v1/query`, `loki/api/v1/query_range`, `loki/api/v1/labels`,
  `loki/api/v1/label/<name>/values`, `loki/api/v1/series`.

Everything else (status, configuration, admin, TSDB, other data source types) is refused. Time
parameters are RFC 3339 or Unix seconds for Prometheus, Unix nanoseconds or RFC 3339 for Loki.
Repeated parameters such as `match[]` are written once per value (URL-encode the brackets as
`match%5B%5D` with curl). Results are `{"status":"success","data":…}`; an error has
`"status":"error"` and `error`.

For every data source type (Prometheus, Mimir, Loki, Elasticsearch, OpenSearch, SQL, CloudWatch) use
Grafana's unified query, `POST $G/api/ds/query` (`query_datasources`). It is a read tool: it runs
under a Viewer token and is not held for approval, unless the person set it to Ask in the account's
actions table.

```json
{"from": "now-1h", "to": "now",
 "queries": [{"refId": "A", "datasource": {"uid": "<ds uid>"}, "rawSql": "SELECT …", "format": "table"}]}
```

```sh
aurora-agent call grafana query_datasources --body @body.json --out /tmp/aurora-agent/q.json
```

Send read statements only, and keep ranges small; the `grafana-query` skill has the body shapes and
how to read the frames.

## Alerting

- `GET /api/prometheus/grafana/api/v1/rules`: Grafana-managed rules grouped by folder and group,
  with `state` (`inactive`, `pending`, `firing`) and their `alerts[]`; filter with `dashboard_uid`,
  `folder_uid`, `state=firing`.
- `GET /api/v1/provisioning/alert-rules` and `…/alert-rules/<uid>`: rule definitions (queries,
  condition, `for`, labels, annotations).
- `GET /api/alertmanager/grafana/api/v2/alerts`: alert instances with `labels`, `startsAt` and
  `status.state` (`active`, `suppressed`); `filter=severity="critical"` narrows.
- `GET /api/alertmanager/grafana/api/v2/silences`: silences with `status.state` (`active`,
  `pending`, `expired`).
- Silencing (held for approval): `POST /api/alertmanager/grafana/api/v2/silences` with
  `{"matchers": [{"name": "alertname", "value": "HighErrors", "isRegex": false, "isEqual": true}],
  "startsAt": "…Z", "endsAt": "…Z", "createdBy": "<agent>", "comment": "why"}`. Sending an `id`
  changes that silence. Ending one: `DELETE /api/alertmanager/grafana/api/v2/silence/<id>`.

## Annotations

`GET /api/annotations?from=<ms>&to=<ms>` (Unix milliseconds) with `dashboardUID=`, `tags=`
(organization-wide annotations), `type=annotation|alert`, `limit=`. Adding one (held for approval):
`POST /api/annotations` with `{"text": "…", "tags": ["deploy"], "time": <ms>, "timeEnd": <ms>}` and
`dashboardUID` plus `panelId` to pin it to a dashboard or panel; without them it is organization-wide.

## The write tools with `aurora-agent`

Each is an operation, so a body file works without `--path` (write the JSON to a file, then):

```sh
aurora-agent call grafana create_annotation --body @annotation.json
aurora-agent call grafana update_dashboard --body @dashboard.json
aurora-agent call grafana create_silence --body @silence.json
aurora-agent call grafana delete_silence id=<silence id>
```

Single fields also work: `text="Deployed v2" tags:='["deploy"]' time:=1763647200000`.
`aurora-agent describe grafana <operation>` lists the fields. All four are held for approval.

## Errors

| Status | Meaning |
|---|---|
| `400` | bad parameters; "UID is invalid" for a UID with characters other than letters, digits, `-`, `_` |
| `401` | the token is invalid or expired: the person reconnects with a new token |
| `403` | the token's role or the folder or data source permissions do not allow it; say which request |
| `404` | no such dashboard, folder, rule or data source (or not visible to this token) |
| `412` | dashboard version mismatch: read again |
| `422` / `502` | the backend refused or failed: its message is in the body |
