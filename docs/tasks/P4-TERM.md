# P4-TERM: Web terminal to the agent's sandbox (wterm + Ghostty core) (owner, 2026-11-10)

## Goal

An interactive shell into a hosted agent's sandbox from the dashboard: a new
**Terminal** tab rendering with **wterm** (https://wterm.dev/, Apache-2.0) and its
**Ghostty core** (`@wterm/ghostty`, libghostty compiled to WASM, ~400 KB), wired
over a WebSocket to a PTY inside the sandbox. The same people who can already use
the agent (read its transcript, restart its computer, run its preview start
commands) can type in its sandbox; nothing more, nothing wider.

The transport reuses the port-preview stream machinery of P4-PRV-2 (D-147,
`16 §W18`): a single-use grant, the runner's stream listener :4108 and the
credit-based W6 frames — with a **PTY target** beside the TCP-port target. The
terminal is a dashboard surface (session cookie, same origin, the existing CSP):
**no relay, no previews domain, no org previews toggle**.

## Spec (source of truth; read, don't re-decide)

- D-161; **SEC-87** in `docs/17` (+ residual risk note).
- `docs/16 §W19` (terminal streams: the `auy_` grant, the redemption extension,
  the `pty.*` frames on W6) on top of `16 §W18` (listener, credit windows,
  chunking — unchanged); `docs/11 §Web terminal` (wake, keep-awake, lifecycle,
  the :4108 policy change); `docs/12 §Terminal` (supervisor PTY posture);
  `docs/04` best-effort audit note (`agent.terminal.open`); `docs/00` (`auy_`);
  `docs/03 §Planned changes` (`terminal_stream_grants`); `docs/05` route;
  `docs/14` Terminal tab; `docs/15` config + Helm + NetworkPolicy (4108 from
  the API too); `docs/02 §Limits`.
- `docs/18 §Web terminal` (tests, kind e2e round trip).

## Scope

Frontend: `@wterm/react` + `@wterm/ghostty` as pinned npm dependencies
(checksum-verified lockfile; **vendored**, no CDN — the CSP allows only
`'self'`), the Ghostty WASM served from `web/public` as a hashed static asset
(`Content-Type: application/wasm`), CSP gains `'wasm-unsafe-eval'` in
`script-src` (required for WASM instantiation; nothing else loosens). New
Terminal tab component with reconnect, resize, IME and copy passthrough
(wterm renders to the DOM: native selection, find and screen-reader support —
the reason this core fits Aurora's a11y target; no canvas).

Backend: `terminal.enabled` (default **off**, like previews — an operator turns
on an interactive path into sandboxes deliberately); the API WebSocket route and
grant minting; `terminal_stream_grants` migration + repo; the runner's
redemption extension (`kind: "terminal"`) and `pty.open` path on the existing
:4108 listener; supervisor `pty.*` handling (spawn the image shell in a PTY as
the agent user, `TIOCSWINSZ` resize, credit flow control shared with streams);
Helm value + NetworkPolicy (4108 reachable from relay **and api** pods only);
`scripts/guards/edge-routes.sh` and the sandbox-network check unchanged for the
sandbox side (a terminal is API-initiated; the sandbox gains no new egress).
Contracts **C-616..C-625** only.

## Acceptance

- API: WS auth matrix (session only; API key, agent token, anonymous → 401;
  BYO agent → 422 `terminal_not_supported`; frozen → 423; disabled → 404
  `terminal_disabled`), grant single-use/TTL, per-user and per-agent caps,
  idle + lifetime close, audit row, no terminal bytes in any log or
  request-log row.
- Runner/supervisor: `pty.open` only from a redeemed terminal grant
  (API-returned sandbox/boot), supervisor refuses when not enabled/shares the
  posture of `12 §Terminal`, resize applied, credit accounting (window bounds
  hold), W6 reconnect closes every PTY, an older supervisor ignores `pty.*`
  (no terminal, harmless).
- Web: tab gating, terminal renders and reconnects, resize round trip, frozen/
  parked notices, wake on open, vitest + Playwright `stage4-routes`/`stage4-axe`
  for the changed screens, CSP render test with `'wasm-unsafe-eval'`.
- kind e2e: open the terminal on a running sandbox, run `echo aurora-terminal`
  and assert the output arrives; freeze closes the session; a parked sandbox
  wakes on open. `scripts/e2e/kind.sh` env overrides (own cluster; ask the
  coordinator first).
- fmt, clippy `-D warnings`, full `aurora-api` (`--test-threads=8`), runner +
  supervisor tests, web typecheck/lint/test/build, guards, helm lint/template
  (terminal on/off), OpenAPI drift + `schema.d.ts`.

## Constraints

- Depends on P4-PRV-2's stream listener and W6 binary-frame machinery (or lands
  with it if sequenced together); the listener, grant hashing and credit
  machinery are **shared, not forked** — a terminal is `kind: "terminal"` on
  the same surfaces.
- The supervisor spawns only its own image shell, as the agent user, cwd
  `/workspace`, container environment **minus every `AURORA_*` variable** and
  minus the boot document (SEC-01/SEC-34 posture); a terminal is never a
  turn, never a conversation event, never `last_engaged_at` on keystrokes
  (only the session-level keep-awake of C-620).
- Terminal bytes are never logged, stored or counted beyond byte totals
  (like `16 §W18`); audit records opens, never content.
- No agent tool can open, resize or close a terminal (SEC-87); the feature is
  people-only, session-cookie only.
- Change files only with the Edit/Write tools; never python, sed, awk, perl,
  `cat >>` or heredocs. No `scripts/perf/*`.
