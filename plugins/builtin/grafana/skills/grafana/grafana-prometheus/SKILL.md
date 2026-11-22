---
name: grafana-prometheus
description: "Read Prometheus and Mimir results through Grafana without being misled by steps and windows; health sweep and window comparison"
---

# Prometheus and Mimir through Grafana

Read `grafana-query` first (finding the UID, times, reading frames). This skill is what to watch for
in the numbers, and the `sweep` and `compare` subcommands of this skill's script:
`P="$SKILLS_DIR/grafana-prometheus/scripts/grafana-prom"` (`--help` on each).

## Peaks, steps and windows

- A range query at step S shows the samples at the step boundaries and misses a peak shorter than S.
  Use `max_over_time(m[S])` at step S, or one instant `max_over_time(m[W])` over the whole window W.
- A point of `max_over_time(m[30m])` at time t covers t-30m to t. Report it as that interval, not as
  the instant t.
- One sample at a cap (busy workers equal to the configured maximum) is not lasting saturation. Check
  how long it lasted with a 1 to 5 minute step before calling it saturation.
- High load with an idle CPU and little `iowait` means tasks in uninterruptible sleep, often waiting
  on a network filesystem. Check `node_procs_blocked` and the CPU modes before blaming the CPU.

## Capacity

`predict_linear(node_filesystem_avail_bytes{<selector>}[7d], 14*86400) < 0` finds a filesystem
expected to fill within 14 days. A shared mount (`ceph`, `nfs`, …) reports the same value on every
host: report it once, with its size.

## Health sweep

Run `sweep` at the start of any service review, before the deep dive: a slow trend (a filesystem
filling over a week) does not show in an anomaly hunt over the requested window, only here.

```sh
"$P" sweep --ds <uid> --selector 'job="node",instance=~"web.*"' --from 2026-11-20T03:00:00Z --to 2026-11-20T15:00:00Z --show-queries
```

One request. It prints a host by check table (the worst value of each check over the window, at
`--to`) and one row per filesystem: size, free now, free %, lowest free in the window, used per day
over `--trend` (default `7d`) and a linear days to full. A `!` marks a value past its flag, or a
filesystem under 10 % free or under 30 days to full. `--last 12h` ends now (use absolute times in a
report). `--host-label` names the host (default `instance`). `--json` prints JSON. Paste the
`--show-queries` output into the report as the evidence.

Add the service's own checks with `--checks file.json`, kept in the agent's workspace:

```json
{"checks": [
  {"name": "workers busy max", "expr": "max by (${HOST}) (max_over_time(app_workers{${SELECTOR},state=\"busy\"}[${WINDOW}]))", "flag": ">=300"},
  {"name": "restarts", "expr": "sum by (${HOST}) (resets(app_uptime_seconds_total{${SELECTOR}}[${WINDOW}]))", "flag": ">0"}],
 "disable_default_checks": ["reboots"]}
```

`${SELECTOR}`, `${WINDOW}` (such as `43200s`) and `${HOST}` are the only placeholders; any other
`${…}` is refused. `replace_default_checks: true` drops the defaults. At most 30 checks; `flag` is
`>`, `>=`, `<` or `<=` and a number. If one expression is invalid Grafana fails the whole request:
read the error, fix it, run again.

## Is this normal?

```sh
"$P" compare --ds <uid> --from 2026-11-20T13:00:00Z --to 2026-11-20T15:00:00Z --offsets 1d,7d 'sum(rate(http_requests_total{job="api"}[5m]))'
```

The window and the same window 1 day and 7 days earlier (one request each), per series
`min p50 p95 max mean points`, and `p95 now / median p95 before`: near 1x is normal for that time, 2x
or more is not. The earlier windows are **outside** the window the person asked about: say so in the
report and give their exact times, which the output prints.
