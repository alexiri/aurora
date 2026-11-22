---
name: grafana-prometheus
description: "Read Prometheus and Mimir results through Grafana without being misled by steps and windows; window comparison and why a metric jumped"
---

# Prometheus and Mimir through Grafana

Use `grafana-query` for the UID, times and frames. This skill: what to watch for in the numbers, and
`compare` of its script: `P="$SKILLS_DIR/grafana-prometheus/scripts/grafana-prom"` (`--help`).

## Peaks, steps and windows

- A range query at step S shows the samples at step boundaries and misses a peak shorter than S. Use
  `max_over_time(m[S])` at step S, or one instant `max_over_time(m[W])` over the window W.
- A point of `max_over_time(m[30m])` at t covers t-30m to t: report that interval, not the instant t.
- One sample at a cap (busy workers at the configured maximum) is not lasting saturation: check how long
  with a 1 to 5 minute step.
- High load with an idle CPU and little `iowait` means tasks in uninterruptible sleep, often a network
  filesystem: check `node_procs_blocked` and the CPU modes before blaming the CPU.

## Is this normal? `compare`

```sh
"$P" compare --ds <uid> --from 2026-11-20T13:00:00Z --to 2026-11-20T15:00:00Z --offsets 1d,7d 'sum(rate(http_requests_total{job="api"}[5m]))'
```

The window and the same window 1 and 7 days earlier (one request each): per series `min p50 p95 max mean
points`, and `p95 now / median p95 before`: near 1x is normal for that time, 2x or more is not. The earlier
windows are **outside** the window the person asked about: say so and give their exact times (printed).

## Capacity

`predict_linear(node_filesystem_avail_bytes{<selector>}[7d], 14*86400) < 0` finds a filesystem expected to
fill within 14 days; `deriv(…[7d])` is the change per second. A shared mount (`ceph`, `nfs`) reports the same
value on every host: report it once, with its size.

## Why did this metric jump?

1. Find the dashboard (`search`, or the UID in a link) and read the panel's queries with `grafana-query panels
   <uid>`; replace left-over variables (`$job`, from `templating.list[].current`) and `$__rate_interval`
   with a concrete value such as `5m`.
2. Run them with `grafana-query promql` (`logql` for Loki) about an hour either side of the time named
   (convert to UTC), expressions that share a window in one call. Split `by (…)` (`instance`, `status`,
   `route`, `pod`) to see which part moved.
3. `compare` with `--offsets 1d,7d`: a daily peak at the same time is not an incident.
4. Causes: annotations in the window (`list_annotations`, also with `dashboardUID`), alerts that fired
   (`list_alert_rule_states`, `list_annotations type=alert`), and log errors around the jump (`logql`).
5. Answer: what changed, when (UTC and the person's time), by how much, which labels carry it, what lines up
   with it, and what you did not find. Link the dashboard at the window.
