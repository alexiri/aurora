# REVIEW-5 wave 2 — SUP (supervisor core) — saved by coordinator

## SUP-1 (medium, CONFIRMED; SEC-24 vs W7 wording) — platform tool calls accepted with no active turn
- `services/aurora-supervisor/src/tools.rs:434-444` (turn_id None sent anyway), `:394` (socket 0666, any `session` string); `services/aurora-api/src/domains/tools/service.rs:56` (`if let Some(turn)`); `tools/messaging.rs:47`; test `services/aurora-api/tests/it/tools.rs:294-298` asserts "Turn id is optional" → 200.
- SEC-24 "tool calls are fenced on the agent's active turns" (docs/17:58, "proven"); docs/12 §Platform tools; docs/16 W7 weaker wording.
- Scenario: a background process (process_start) calls a messaging tool after the turn ended.
- Fix: require an active turn in `identity`; supervisor refuses before sending; W7 "turn_id is required". Note: TOOL-10 (freeze) is the same function.

## SUP-2 (medium; code path confirmed) — no PID-1 reaper: orphans become zombies and exhaust the sandbox's 512 pids
- `services/aurora-supervisor/src/main.rs` (no SIGCHLD/waitpid); `adapters/claude_code/launcher.rs:55,98`; `process.rs:369`; `services/aurora-runner/src/backends/podman.rs:280` (`init: Some(false)`); docs/12 §Process model item 1 "Runs as PID 1 (uid 1000), reaps children, forwards SIGTERM to process groups".

## SUP-3 (low) — reads agent-writable files with no size bound (`harvest.rs:177`, `:203`; `tools.rs:255`, `:477`; `home.rs:243,256,265,388`; `supervisor.rs:253`)
## SUP-4 (low) — process table unbounded; `max_running` check-then-act (`process.rs:345-369`)
## SUP-5 (low) — tools socket / MCP bridge spawn unbounded concurrent calls; no line-length bound (`tools.rs:534-562`; `mcp_bridge.rs:146-162`, `:151`)
## SUP-6 (low) — boot accepts `wss://` runner URLs the binary cannot dial (no TLS feature: `Cargo.toml:96`; `boot.rs:141`); W6 plaintext not recorded as a residual risk in docs/17
## SUP-7 (low, spec) — docs/12 says the supervisor scrubs AURORA_BOOT from its environment; it cannot (relies on non-dumpability, `boot.rs:3-6`)
## SUP-8 (low) — secret-name filter gaps (`spawn.rs:23`, `:26`: DATABASE_URL, *_DSN, *_PASS, *_PWD, *_AUTH, *_APIKEY, *_CREDENTIAL; value prefixes only aub_/aup_); `#[derive(Debug)]` on `proxy.rs:24 Credential`, `pgrelay.rs:40 DbTurns`
## SUP-9 (low, accepted-residual candidate) — `/run/aurora` agent-owned: agent can replace `ca.pem` and `tools.sock` (`podman.rs:41`, `:218-225`; `kubernetes/objects.rs:315`) — supervisor's own trust is in memory
## SUP-10 (low, PLAUSIBLE) — Claude result mapping treats any `result` without `is_error` as success, ignores `subtype` (`adapters/claude_code/stream.rs:273-291`)

## Sound: non-dumpable right after reading boot var (`boot.rs:176-184`, `main.rs:94`); PID 1 signal protection; all spawns through spawn.rs with real-child SEC-34 test; no fd inheritance; channel tokens memory-only; proxies loopback-only, strip Proxy-Authorization; DB relay credential in memory; Claude adapter argv-inline MCP config (`--strict-mcp-config`), writes nothing to home; projection path confinement.
## Not covered: instructions.rs every render path; inbox.rs symlink following; home.rs commit symlinks; outbound.rs TOCTOU; logging redact.
