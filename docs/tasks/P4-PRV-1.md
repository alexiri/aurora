# P4-PRV-1: Port previews — shares, wake and on-demand start

## Goal
Implement D-144/D-145 and SEC-80 (SPEC-P4c): preview shares per (agent, port) with stable random hostnames under `previews.domain`, `private` visibility only, wake and on-demand start. Off unless the instance sets `previews.enabled` + a valid `previews.domain` and the org enables Port previews.

## Spec (source of truth; read, don't re-decide)
- D-144, D-145; SEC-80 (+ residual) in `docs/17`.
- `docs/11` §Port previews (share model, hostname, visibility, who may act, start command, discovery, allow-list, wake, keep-awake reason `preview`, lifecycle, audit); `docs/12` §Previews; `docs/16` (`preview.start` work item, `previews` home-sync part); `docs/03 §Planned changes` (`preview_shares`, `sandboxes` preview columns, `organizations.settings.previews_enabled`); `docs/04` matrix + freeze; `docs/05` (due-work arms, Port previews routes, background job); `docs/10`; `docs/14` (agent Previews tab, org Port previews page); `docs/15` (config keys, §Port previews operations); `docs/02` limits.
- `docs/18` §Port previews (the tests that do not need the relay; the relay is P4-PRV-2).

## Scope
Migration + repos (move rows out of `03 §Planned changes`); API routes, matrix rows, audit, wake demand (rate-limited, never wakes a frozen agent); supervisor/runner: in-memory share list, `preview.start` with no command text on the wire, keep-awake `preview` with its cap; web tabs/pages; config validation of `previews.domain` (disjoint from dashboard host, trusted origins, gateway host). Contracts **C-540..C-549** only. No relay, DNS, TLS or Ingress change here.

## Acceptance
Every non-relay test in `18 §Port previews`; SEC-80 `proven`; full `aurora-api` (`--test-threads=8`), supervisor/runner tests, `AURORA_TEST_PODMAN=1 scripts/e2e/hosted.sh` for the start/wake path (ask the coordinator first), web checks + Playwright, fmt, clippy `-D warnings`, guards, OpenAPI drift test.

## Constraints
Agent tools can never create or change a share. Change files only with the Edit/Write tools. No `scripts/perf/*`.
