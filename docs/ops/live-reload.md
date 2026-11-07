# Live reload for development (OPS-018, phase 1)

Goal: saving a file updates the running dev stack. Decision: I-43 (`docs/90-decisions.md`). Measured on the
dev host (16 CPUs, rootless podman, 2026-11-07).

## What you get

| | command | what happens on save |
|---|---|---|
| Dashboard | `just dev` | `next dev` (Turbopack) Fast Refresh, state kept. `/v1/*` is rewritten to the API, so the page, the cookies and the CSP stay same-origin. |
| API, gateway, hook host, runner | `just dev` | `scripts/dev/live.py` rebuilds only the affected binaries (one `cargo build -p …`), then restarts those services. The old process keeps serving while the new one compiles; a failed build prints the compiler output and leaves it running. |
| kind cluster | `just kind-live` | the same watcher runs `kind-dev` for the affected services (OPS-017: builder container, thin image, `kind load`, rollout restart), about 35 s. Web: use `just dev`. |
| Sandbox supervisor | `runner.dev_supervisor_binary` | a host-built supervisor is bind-mounted into new podman sandboxes; no image rebuild. |

`just dev` runs Postgres (`just dev-db`), `just migrate`, mints the dev CA once, then the loop. Logs of every
process are multiplexed with a per-service prefix, and each restart prints `<service>: serving on :PORT N.NNs after
save`. Ctrl-C stops every process group. The runner uses the `fake` backend; set `AURORA__RUNNER__BACKEND=podman`
(plus the usual podman prerequisites) for real sandboxes. Restarting the runner drops in-flight turns, which is
acceptable in development only. `deploy/dev/env.sh` exports `AURORA_BOOTSTRAP_RUNNER_TOKEN`, so the API registers
the dev runner at start (without it the runner exits with `Unknown runner token`).

Knobs: `AURORA_DEV_PROFILE` (cargo profile; the justfile default is `dev-fast`, which compiles workspace crates
incrementally, I-36), `AURORA_LIVE_SERVICES` (default `api,gateway,hookhost,runner`), `AURORA_LIVE_WEB=0`.

## Measurements

"Save" is the file change; "serving" is the first successful TCP connect to the service port (the log line the
loop prints). The compile is incremental (workspace crates only), warm, in the default `dev` profile with
`CARGO_PROFILE_DEV_INCREMENTAL=true` (so no extra profile directory; the repo's `dev` profile has incremental
off, I-24, while `dev-fast` has it on). **The host was busy with other lanes (load average 15 to 17 on 16 CPUs)
throughout, so every number below is pessimistic.** One-line comment appended to a leaf module.

| Edit | rebuild (cargo) | save → serving |
|---|---|---|
| `aurora-hookhost` `redact.rs` (warm) | 3.4 s | **3.7 s** |
| `aurora-runner` `reporter.rs` (warm) | 3.3 s | **3.7 s** |
| `aurora-api` `jobs.rs` / `audit.rs` (warm; 4 runs) | 13.0, 14.5, 15.4, 15.5 s | 15.0, 16.5, 17.3, 16.5 s |
| `aurora-gateway` `hold.rs` (first edit, cold incremental cache) | 94 s | 94 s (not re-measured warm) |
| first edit of each crate after a fresh build | 56 to 130 s | the incremental cache is created once per crate |
| web Fast Refresh | | the HMR websocket (`/_next/hmr`) is accepted through `proxy.ts` and `built` events arrive within about 0.1 s of the save |

Of the 11 s `aurora-api` cargo time in a `--timings` run, the crate compile was 8.8 s and the link 2.0 s.
The restart itself (stop old, start new, accept connections) costs 0.2 to 1 s.

**Target (< 5 s for a leaf edit): met for the hook host and the runner, not for the API (about 15 s) and not
re-measured for the gateway.** The gap is the size of `aurora-api` (one large crate, so even an incremental
rebuild regenerates a lot of code) on a host with every core busy. Levers, not taken here: split `aurora-api`
into smaller crates (an architecture change), the Cranelift backend (nightly), a quiet machine. The web
measurement was a headless check (the websocket handshake and `built` events), not a browser session, so the
"state kept" part of Fast Refresh is Next's documented behaviour, not something this test observed.

## What was evaluated

- **True hot patching** (Subsecond/Dioxus-style, `cargo-hot`-like tools): these patch UI-framework functions in
  a running process. Our services are tokio/axum servers with global state, pools and listeners; none of the tools
  targets that, so not used.
- **Socket handover** (`systemfd`/`listenfd`, or a supervisor holding the listeners): needs listener-inheritance
  code in four services. The window it would close is the restart, 0.2 to 1 s here, because the old process
  serves until the new binary exists. Not adopted; `live.py` could be extended later.
- **`watchexec`, `bacon`, `cargo-watch`**: they restart one command, not "rebuild only the affected binaries of
  four services and measure the time to listening". `live.py` needs only Python 3 (standard library), so **there
  is nothing to install**. If `python3` is missing, `just dev` and `just kind-live` stop with an install hint.
- **Tilt for kind**: its `live_update` syncs a binary into the pod, but host-built binaries cannot run in the
  distroless images (the host glibc is newer, I-42), so the sync would have to come from the builder container,
  which is what `kind-dev` already does. Not adopted: it would add an install without a speed gain.
  `just kind-live` reuses `kind-dev` instead (`kubectl`, `kind`, `helm` and `podman` are required and checked).
- **Restart idempotence**: migrations are applied by `just migrate` (re-running is a no-op); the services
  re-subscribe their LISTEN/NOTIFY channels on start; the runner re-registers with the same bootstrap token.

## Supervisor bind-mount (dev only)

```toml
[runner]
backend = "podman"
dev_supervisor_binary = "/home/me/aurora/target/debug/aurora-supervisor"
```

Build it with `cargo build -p aurora-supervisor`, then restart the sandbox; new sandboxes run the host binary,
mounted read-only at `/usr/local/bin/aurora-supervisor`, the image entrypoint.

Guard (`aurora-config`, tested in `crates/aurora-config/tests/config.rs`): the runner refuses to start unless
the backend is `podman`, `server.bind` is a loopback address, `server.external_url` and `runner.api_url` are
`localhost`/loopback URLs, and the path is absolute with no `:`, `,`, `..` or control characters. The runner also
fails when the file does not exist and logs a warning on every start. The mount is never offered on Kubernetes
(use `just kind-dev sandbox`). **Never set it in a deployment.** The host binary must run in the sandbox image:
the image's glibc has to be at least the host's, otherwise build in a container or use the image rebuild.

## Web in dev

`proxy.ts` already allows what Next dev needs: `'unsafe-eval'` and inline styles in development only, and
`connect-src 'self'` covers the same-origin HMR websocket. No web change was needed. Open the dashboard at
`http://localhost:4100`; other host names need `allowedDevOrigins` in `next.config.ts` (Next 16 blocks
cross-origin dev requests).
