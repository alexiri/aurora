---
name: grafana-alerts
description: Use when an agent should create, change, list or delete Grafana alert rules, for example to be woken when a problem comes back
---

Manage Grafana alert rules only through `scripts/grafana-alert` (see `--help`): `create`, `list`, `update`, `delete`, `expire`. Every rule carries the guard labels `managed_by` and `expires` (at most 30 days; an extension needs `--renew --reason`), and change and delete refuse a rule without the agent's label or outside the given folder — never touch a rule the agent did not make. Caps: 10 managed rules per folder and 5 per `--service`; one Prometheus query, `--for` at least 2m, a range of at least 5m, and a receiver a person made. To change the query or threshold, delete and create again. Every write is held for a person's approval; the script is help, not enforcement. A rule costs a query every interval: few and short-lived.
