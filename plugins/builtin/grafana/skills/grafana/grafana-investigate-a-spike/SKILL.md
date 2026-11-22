---
name: grafana-investigate-a-spike
description: "Find out why a metric jumped"
---

# Find out why a metric jumped

Use this for "why did the error rate jump at 14:00?" or "what happened to latency last night?".
`$G` is the Grafana address shown with the account in `app.md` (it may end with a path such as
`/grafana`). Aurora adds the token: send none, no `-k`. Prefer `aurora-agent call grafana …` so large
answers are saved to a file instead of printed.

## 1. Find the dashboard

- The person gave a link `…/d/<uid>/<slug>?…`: the UID is the part after `/d/`.
- Otherwise search: `aurora-agent call grafana search query="api errors" type=dash-db` (or
  `GET $G/api/search?query=api%20errors&type=dash-db`). Each hit has `uid`, `title`, `url`, and
  `folderTitle`; pick by title and folder, and ask when two look alike.

## 2. Read the panel's queries

`aurora-agent call grafana get_dashboard uid=<uid> --out /tmp/aurora-agent/dash.json`, then read
`dashboard.panels[]` (rows nest more under `panels[].panels[]`). For the panel the person means
(match `title`), take:

- `datasource.uid` (and `datasource.type`: `prometheus`, `loki`, …). A value like `${datasource}`
  is a dashboard variable: its current value is in `dashboard.templating.list[]` (`current.value`).
- `targets[].expr` (PromQL or LogQL). Replace dashboard variables (`$job`, `${instance}`) with the
  values from `templating.list[].current`, and `$__rate_interval` / `$__interval` with a concrete
  window such as `5m`.

## 3. Run the queries over the window

Take about an hour either side of the time the person named (convert their time zone to UTC).
Prometheus or Mimir (Loki is the same under `loki/api/v1/query_range`):

```sh
aurora-agent call grafana prometheus_query_range uid=<ds uid> \
  query='sum by (status) (rate(http_requests_total{job="api"}[5m]))' \
  start=2026-11-20T13:00:00Z end=2026-11-20T15:00:00Z step=60s
```

Keep `(end - start) / step` under about 1000 points. Look for when the value changes and which
label values (`by (…)`) carry the change: split by `instance`, `status`, `route` or `pod` to find the
part that moved.

## 4. Compare with the day before

Run the same query with `start` and `end` 24 hours earlier, or wrap the expression in
`offset 1d`. A daily peak at the same time is not an incident; say so.

## 5. Look for causes

- Annotations (deployments, incidents) in the window: `GET $G/api/annotations?from=<ms>&to=<ms>`
  (Unix milliseconds), also with `dashboardUID=<uid>`.
- Alerts that fired: `GET $G/api/prometheus/grafana/api/v1/rules?dashboard_uid=<uid>` for the
  dashboard's Grafana-managed rules, `GET $G/api/alertmanager/grafana/api/v2/alerts` for what fires
  now, and `GET $G/api/datasources/proxy/uid/<ds uid>/api/v1/rules` for the data source's own rules.
- Logs around the jump: `loki_query_range` with the service's stream selector and `|= "error"`,
  `limit=50`.

## 6. Answer

Say what changed, when (UTC and the person's time), by how much, which label values carry it, and
what lines up with it (a deployment annotation, an alert, log errors). Say what you did not find.
Link the dashboard at the window: `$G/d/<uid>?from=<start ms>&to=<end ms>`, adding
`&viewPanel=<panel id>` for one panel.

## Pitfalls

- `403`: the token's role or the folder or data source permissions do not cover it; say which
  request failed. A `400` "UID is invalid" means the UID has characters other than letters, digits,
  `-` and `_`.
- Only the proxy paths listed in `tools.md` work; other backend paths (status, admin, TSDB) are
  refused.
- Never add an annotation or silence an alert while investigating unless the person asks.
