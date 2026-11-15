# Web terminal (phase 4)

The **Terminal** tab on a hosted agent's page opens an interactive shell in the agent's sandbox, for the people who can
already run commands there (`can_use` on the workspace). It is **off by default** and a single instance switch turns it
on. Design: [docs/15 §Web terminal operations](../15-deployment-operations.md#web-terminal-operations-phase-4-p4-term-d-161-sec-87-fix-061),
`docs/16 §W19` (protocol), `docs/API_CONTRACTS.md` C-616..C-626 (routes, errors), SEC-87 (`docs/17`).

## Enable

```yaml
terminal:
  enabled: true      # the single switch; it renders config.terminal.enabled
```

Set only this chart value: it mirrors `config.terminal.enabled`, and the chart refuses a render where
`config.terminal.enabled=true` stands alone or disagrees with it. Turning it on:

- renders `config.terminal.enabled = true` for the API (while off, every terminal route answers 404 `terminal_disabled`);
- opens the runner's **stream listener on :4108** (the same listener previews use; either switch opens it) and the runner
  registers the `terminal` capability. Runners and sandbox images must be of the same release as the API: an older runner
  closes the socket `4502` (an older supervisor ignores `pty.*`: roll the sandbox image with the runner);
- renders the NetworkPolicy flow **api → runner:4108** (the terminal's leg 1) beside the relay's flow when previews are on.

No workload is added: the browser's WebSocket ends on an **API** pod, which dials the runner. There is no relay, no
preview domain and no new DNS or certificate. Tuning keys (all `config.terminal.*`): `awake_cap_secs` (default 14400, at
most `hosted.background_cap_secs`), `wake_window_secs` (60), `wake_wait_secs` (45).

Enabling on an existing deployment is one `helm upgrade --set terminal.enabled=true`; no migration beyond the normal
release order (see [docs/15 §Upgrades](../15-deployment-operations.md#upgrades)).

## Flows

```
browser ──wss /v1/agents/{agent}/terminal (session cookie, Origin checked)──▶ API
API ──POST /preview-streams/open {kind:"terminal", grant}──▶ runner :4108   (leg 1, WebSocket)
runner ──pty.* frames on the sandbox's supervisor channel──▶ supervisor      (leg 2)
supervisor ──PTY (agent user, cwd /workspace, no AURORA_* environment)──▶ shell
```

The API mints a single-use `auy_` grant bound to (agent, sandbox, boot) that expires in 30 s and never reaches the
browser; the browser names neither sandbox nor boot. API keys, agent tokens and cross-origin upgrades are refused 401. A
bring-your-own agent answers 422 `terminal_not_supported`; a frozen agent 423. Bytes are never logged, stored or
request-logged; the audit row `agent.terminal.open` carries metadata only. A terminal open on a parked sandbox wakes it
(`TERMINAL_WAKES_PER_AGENT_HOUR` 12) and keeps it awake while in use.

## Close codes

| Code | Meaning |
|------|---------|
| 1000 | normal close (the shell exited or the user closed the tab) |
| 4403 | access lost: the API re-checks the holder every 10 s (session, org membership, `can_use`, agent not frozen) |
| 4404 | the sandbox or binding is gone |
| 4409 | a live-session cap was reached (below) |
| 4502 | the runner refused the grant (older runner, unknown kind, stale boot) |
| 4503 | the sandbox's channel was lost (restart, park, harness change, runner move); the tab shows "Waking…" and reopens |
| 4504 | idle (600 s) or lifetime (4 h) timeout |

## Caps

`TERMINALS_PER_USER` (2) and `TERMINALS_PER_AGENT` (4) are counted **per API replica** in memory (released on every
exit path; a restart starts at zero), so with N API replicas the effective ceilings are N× those numbers.
`TERMINALS_PER_SANDBOX` (2) is enforced at the runner across replicas. Over a cap the upgrade completes and the socket
closes 4409. Window sizes are 128 KiB with 16 KiB chunks; a slow browser holds output back, nothing is dropped.

## The edge and WebSockets

The terminal needs no extra route (it is `/v1/agents/{agent}/terminal` on the dashboard origin) but the proxy in front
of the cluster **must pass WebSocket upgrades**. A common failure: an nginx edge that proxies with
`proxy_set_header Connection "";` (the keepalive idiom) strips the browser's handshake, the dashboard renders and every
WebSocket feature dies with nothing in the API logs. Working posture:

```nginx
# http context
map $http_upgrade $connection_upgrade {
    default upgrade;
    ''      close;
}
# server/location context, on every location proxying /v1 (or `/`):
proxy_http_version 1.1;
proxy_set_header Upgrade $http_upgrade;
proxy_set_header Connection $connection_upgrade;
proxy_buffering off;
proxy_read_timeout 600s;
proxy_send_timeout 600s;
```

Timeouts of at least 600 s avoid the visible reconnect of an idle terminal (the tab reconnects by itself). Under
`http2 on` browsers open the WebSocket on a separate HTTP/1.1 connection; see
[docs/15 §Port previews](../15-deployment-operations.md#port-previews-phase-4-p4-prv-1-p4-prv-2-d-144d-147-sec-80sec-82-sec-85)
for the full recipe.

## Troubleshooting

| Symptom | Likely cause |
|---------|--------------|
| No Terminal tab | `terminal.enabled` is off, the agent is bring-your-own, or the person lacks `can_use` |
| Tab shows an error at once, 404 `terminal_disabled` | the instance switch is off |
| Closes with 4502 | runner older than the API, or the runner's 4108 is not reachable from the API (NetworkPolicy; an external runner needs a path to the API) |
| Reconnects every few minutes | the edge's read timeout is shorter than 600 s |
| Never connects, no API log line | the edge strips the `Upgrade` header |
| 4409 immediately | the per-user or per-agent cap on that API replica |

## Metrics

Terminal traffic exposes byte totals only, never content, and there is no terminal-specific series: the shared runner
stream listener and the API's request metrics apply. The audit log has one `agent.terminal.open` row per open.
