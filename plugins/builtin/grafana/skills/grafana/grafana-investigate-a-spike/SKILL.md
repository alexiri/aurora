---
name: grafana-investigate-a-spike
description: "Find out why a metric jumped: the dashboard's queries, the same queries over the window and the day before, annotations, alerts and logs"
---

# Find out why a metric jumped

Use this for "why did the error rate jump at 14:00?" or "what happened to latency last night?".
`$G` is the Grafana address shown with the account in `app.md` (it may end with a path such as
`/grafana`). Aurora adds the token: send none, no `-k`. Read `grafana-query` first: it explains the
query route and the helper script used below (`S="$SKILLS_DIR/grafana-query/scripts/grafana-query"`).

## 1. Find the dashboard

- The person gave a link `…/d/<uid>/<slug>?…`: the UID is the part after `/d/`.
- Otherwise search: `aurora-agent call grafana search query="api errors" type=dash-db` (or
  `GET $G/api/search?query=api%20errors&type=dash-db`). Each hit has `uid`, `title`, `url`, and
  `folderTitle`; pick by title and folder, and ask when two look alike.

## 2. Read the panel's queries

`"$S" panels <dashboard uid>` prints each panel with its data source (a `${datasource}` variable
already resolved) and its targets' `expr` (PromQL or LogQL), `query`, `rawSql`. For the panel the
person means (match the title), note the data source UID and type and the expression. Replace the
dashboard variables left in the expression (`$job`, `${instance}`) with the values in
`dashboard.templating.list[].current` (`aurora-agent call grafana get_dashboard uid=<uid> --out
/tmp/aurora-agent/dash.json`), and `$__rate_interval` / `$__interval` with a concrete window such as `5m`.

## 3. Run the queries over the window

Take about an hour either side of the time the person named (convert their time zone to UTC). Both
expressions that share a window go in **one** call (one refId each); Prometheus or Mimir:

```sh
"$S" promql --ds <ds uid> --from 2026-11-20T13:00:00Z --to 2026-11-20T15:00:00Z --step 1m \
  'sum by (status) (rate(http_requests_total{job="api"}[5m]))' 'sum(rate(http_requests_total{job="api"}[5m]))'
```

For Loki use `logql` (same options, `--limit` for lines). The script refuses more than 11,000 points
per series. Look for when the value changes and which label values (`by (…)`) carry the change: split
by `instance`, `status`, `route` or `pod` to find the part that moved.

## 4. Compare with the day before

A second call with the same expressions and `--from`/`--to` 24 hours earlier (or wrap the expression
in `offset 1d`). A daily peak at the same time is not an incident; say so.

## 5. Look for causes

- Annotations (deployments, incidents) in the window: `aurora-agent call grafana list_annotations
  from=<ms> to=<ms>` (Unix milliseconds), also with `dashboardUID=<uid>`.
- Alerts that fired: `list_alert_rule_states` (filter `dashboard_uid=<uid>`) for the dashboard's
  Grafana-managed rules, and `list_alerts` for what fires now. Alert annotations
  (`list_annotations … type=alert`) show past state changes with their values. The data source's
  own rules (`prometheus_rules`) go through the proxy: if it answers 3xx, report it as not
  reachable and move on.
- Logs around the jump: `"$S" logql --ds <loki uid> --from … --to … --limit 50 '{app="api"} |= "error"'`
  with the service's stream selector.

## 6. Answer

Say what changed, when (UTC and the person's time), by how much, which label values carry it, and
what lines up with it (a deployment annotation, an alert, log errors). Say what you did not find.
Link the dashboard at the window: `$G/d/<uid>?from=<start ms>&to=<end ms>`, adding
`&viewPanel=<panel id>` for one panel.

## Pitfalls

- `403`: the token's role or the folder or data source permissions do not cover it; say which
  request failed. A `400` "UID is invalid" means the UID has characters other than letters, digits,
  `-` and `_`.
- When one expression fails, Grafana answers HTTP 400 for the whole call; the script retries each
  one alone, so the output still shows the others.
- Never add an annotation or silence an alert while investigating unless the person asks.
