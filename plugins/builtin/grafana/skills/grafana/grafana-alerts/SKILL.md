---
name: grafana-alerts
description: Use when an agent should create, change, list or delete Grafana alert rules, for example to be woken when a problem comes back
---

Manage Grafana alert rules only through `scripts/grafana-alert` (see `--help`): `create`, `list` (shows each rule's kind), `update`, `delete`, `expire`. A rule is one Prometheus query (`--expr`, a range of at least 5m, at most 200 characters) or one OpenSearch log count (`--logs`, a Lucene query). A stored rule with a longer expression can be listed (kind `?`) and deleted, but not changed or renewed: delete it and create it again with a shorter one.

```
grafana-alert create --managed-by night --folder F --group G --title "p95 over 2s" --ds PROM_UID \
  --expr 'max(rate(x_seconds_sum[5m]))' --op gt --threshold 2 --for 5m --receiver oncall --expires 14d
grafana-alert create --managed-by night --folder F --group G --title "web 500s" --ds OPENSEARCH_UID \
  --logs 'status:500 AND service:web' --window 15m --op gt --threshold 25 --for 2m --receiver oncall --expires 14d
```

Log rules: `--metric` is `count` (default), `sum:F`, `avg:F`, `max:F` or `min:F`. Each evaluation counts the `--window` (5m-60m) ending `--delay` ago (0m-15m, default 1m, for ingestion lag), in `--hist` buckets (default 1m, at most 60). The reduce is fixed so the threshold means the whole window however buckets align: `count` and `sum` add up (the total), `max` and `min` take the extreme, `avg` the mean of the bucket averages. `--time-field` only if the data source's default is wrong. The query is at most 200 characters, without regular expressions or leading wildcards (quote a text with `/`).

No matching lines is zero for `count` and `sum`, so **an `lt` count or sum fires when the logs stop**: use `lt` only for that. For `avg`, `max` and `min` with no matching lines at all, the result depends on the server: Grafana 11.0 fires with no value (NaN), 13.3 stays normal, so never rely on such a rule to report silence.

Every rule carries the labels `managed_by` and `expires` (at most 30 days; an extension needs `--renew --reason`; read from the `expires` label, and a rule whose two copies disagree is refused). Change and delete refuse a rule without the agent's label or outside the given folder: never touch a rule the agent did not make. `list` without `--folder` spans all folders, your rules only. Change also refuses a rule that left the script's shape: a recording rule, a second or different query, or another label, annotation or receiver setting. To change the query or threshold, delete and create again. Caps: 10 managed rules per folder, 5 per `--service`; `--for` at least 2m; a receiver a person made. Every write is held for approval; never send a hand-built body (the gateway refuses one the script cannot build). A rule costs a query every interval, a log rule one aggregation over its window: few and short-lived.
