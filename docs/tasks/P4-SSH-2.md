# P4-SSH-2: `aurora-terminator`, SSH sessions and the exec path into the sandbox

## Goal
`ssh <agent id>@<ssh host>` with a P4-SSH-1 certificate gives a person with access a shell, remote commands, `sftp` and `scp` in the agent's sandbox — through a new certificate-only SSH server that holds no signing key and no cluster credential, sessions decided and revocable by the API, and the web terminal's runner :4108 path (D-239(e)–(j)).

## Linked requirements
- D-239; C-793, C-794, C-795 (sessions UI, Admin › Overview SSH line), C-796, C-797 (config, secrets, Helm, NetworkPolicy), C-799 (`docs/API_CONTRACTS.md` §Phase 5 — P4-SSH); SEC-92, SEC-94, SEC-95, SEC-96 (`docs/17`) and the three SSH residual risks.
- `16 §W20` (normative wire: SSH protocol rules, terminator plane, leg 1, leg 2 `exec.*`), `12 §SSH channels`, `11 §SSH access` and §Keep-awake, `04 §SSH access` points 2–4, `05 §SSH access` (P4-SSH-2 rows) and `§Terminator plane`, `03 §Planned changes` (`ssh_sessions`, `ssh_stream_grants`, `ssh_terminator_replicas`, `sandboxes.ssh_*`), `02 §Limits` (the other `SSH_*`), `15 §SSH access`, `23 §Values planned` (SSH), `18 §SSH access`, `00` (`aut_`, `auz_`), `01` (component row, trust boundary 14).
- Migrations: `20261201000202_ssh_sessions.sql`, `…000203_ssh_stream_grants.sql`, `…000204_ssh_terminator_replicas.sql`, `…000205_sandboxes_ssh_awake.sql` (spare: `000206..209`).

## Context
- The path you extend, end to end: API `domains/terminal/service.rs` (grant mint, `demand_wake`, keep-awake), `domains/runner_plane/preview_streams.rs` (redemption by prefix: `aux_` port, `auy_` terminal → add `auz_` ssh), `domains/relay/routes.rs` (`/runner/terminal/keep-awake`); runner `services/aurora-runner/src/stream_listener.rs` (leg 1, limits, `with_limits` for tests), `src/api.rs`, `src/channel/**`; supervisor `src/pty.rs`, `src/pty_exec.rs` (the safe `setsid`/`TIOCSCTTY` bootstrap), `src/streams.rs`, `src/spawn.rs` (`child_env` + the `AURORA_*` sweep); protocol `crates/aurora-protocol/src/{runner,supervisor}.rs` (`PtyOpen`, `PtyCredit`, …).
- The stateless edge service to copy shape from: `services/aurora-relay/**` (register/heartbeat against an anchor plane, ops port, metrics, Containerfile entry, Helm workload + policies, FIX-069/FIX-102 conventions).
- P4-SSH-1's `domains/ssh/**`, `can_ssh` in `aurora-db::authz`, the `SshCaSigner` (CA public keys for `register`), `GET /v1/ssh` (replace its empty host-key function with the replicas query).
- SSH server library: `russh` (server, `ssh-key` re-export for certificate parsing). Verify the version supports the server-side certificate callback and the algorithm restrictions of `16 §W20`; if a requirement cannot be met with the library, stop and report (do not weaken the rule).
- OneCLI `docs/11` (`/home/alex/agent-platform/onecli-spec/docs/11-ssh-front-door.md`) for the auth state machine, banner/goodbye ordering and lease semantics.

## Scope
### In scope
**Part A — API (Rust)**: migrations + real `docs/03` sections; terminator plane C-794 (anchor from `AURORA_BOOTSTRAP_TERMINATOR_TOKEN`, hash in memory, own rate class); session open as one transaction (certificate re-verification with the CA public keys, key and key-id binding, `can_ssh`, caps, wake demand via `terminal_wake_at` and the shared `terminal_wake` budget); channel grants (`ssh_stream_grants`, `auz_` in `aurora-core::token`); heartbeat revocation (closes rows at detection) and close exactly once (guarded transition, one `agent.ssh.close` row); stale-session sweep; runner redemption `kind: "ssh"` and keep-awake `kind: "ssh"` (`ssh_active_until`/`ssh_awake_since`, keep-awake reason `ssh` in the predicate and in `keep_awake_reasons`); runner registration config `ssh_enabled`; C-793 routes; retention of closed sessions (30 days) and grants (24 h); OpenAPI regenerated.

**Part B — runner and supervisor**: `auz_` legs on :4108 (first-frame `exec.open` within 5 s, frame validation, `SSH_CHANNELS_PER_SANDBOX`, keep-awake stamping, `ssh` capability, :4108 opened when SSH is enabled); `exec.*` frames in `aurora-protocol` (schemas regenerated with `UPDATE_SCHEMAS=1 cargo test -p aurora-protocol`); supervisor exec engine per `12 §SSH channels` (three modes, pipes with stdout/stderr tags, stdin EOF, exit status/signal, env allow-list, fixed `sftp-server` path, CLOEXEC, process-group kill, limits). Sandbox images: `openssh-server` in `images/sandbox/Containerfile` and the e2e `sandbox-min` image (for `sftp-server` only; assert `sshd` is not started).

**Part C — `services/aurora-terminator`** (new crate + image): SSH server per `16 §W20` (algorithms, certificate-only hint-free auth, pre-auth limits, channel/request allow-list, notices on stderr, banner/goodbye ordering, exit propagation), terminator-plane client (register, heartbeat every 30 s with fail-closed after 3 failures, close reporting), leg-1 client to the runner (`runner_endpoint` from the grant answer), idle/lifetime timers, metrics of `15 §SSH access` on 4194, config `[terminator]` and `AURORA_SSH_HOST_KEY` (both key formats), graceful shutdown (`terminator_shutdown` notices).

**Part D — Helm, kind, e2e**: values and templates of `23 §Values planned` (SSH) moved into real `docs/23` sections; `secrets` hook generates `AURORA_TERMINATOR_TOKEN`, `AURORA_SSH_CA_KEY`, `AURORA_SSH_HOST_KEY` (seed format, never overwriting); CA key into api pods only, host key into terminator pods only; NetworkPolicies of `15 §SSH access` (`networkPolicy.sshFrom` required); `aurora-terminator` image in `images/services` and the publish list; `deploy/kind` NodePort 2222 and an `--ssh` switch in the dev/e2e values; new `scripts/e2e/ssh.sh` (uses the e2e lock; gated suites use `require_or_skip`).

**Part E — dashboard**: Account › SSH keys "Active SSH sessions", the agent SSH page's sessions list and status-rail link, Admin › Overview SSH line (C-795).

### Out of scope
Port/agent/X11 forwarding, FIDO keys, a KMS signer, PROXY protocol, an org switch, recording or logging session content.

## Constraints / file ownership
- Owns: new `services/aurora-terminator/**`; `services/aurora-api/src/domains/ssh/**` (sessions, plane), `domains/runner_plane/preview_streams.rs` (the `auz_` branch), `domains/terminal/{mod,service}.rs` (keep-awake `kind`, shared wake helper only); `crates/aurora-protocol/src/{runner,supervisor,terminator}.rs` + `docs/schemas`; `services/aurora-runner/src/{stream_listener,api}.rs`, `src/channel/**` (exec legs); `services/aurora-supervisor/src/{exec.rs (new),pty.rs,pty_exec.rs,streams.rs,spawn.rs}`; the four migrations and `docs/03`; `crates/aurora-core` (limits, token prefix `auz_`, audit actions); `images/**`, `deploy/helm/**`, `deploy/kind/**`, `scripts/e2e/ssh.sh`, `scripts/guards/{limits-gaps.allow,sandbox-network.sh (if a check must learn the terminator)}`; `docs/23`; `web/openapi.json`, schema, the account/agent SSH components of P4-SSH-1, `web/src/components/admin/**` overview line.
- Runs alone in its lane: it touches the runner, supervisor and chart, which other lanes also edit (PHASE5-PLAN schedules it).
- No new privilege anywhere: the terminator pod has no service-account token, no database URL, no Vault; the sandboxes namespace gains nothing (`sandbox-network` guard unchanged and passing).
- Bytes, commands and certificates are never logged; metrics labels carry no ids, addresses or usernames.

## Acceptance criteria (failing-first: each new test fails before its code)
1. API `ssh_sessions_*` ITs: plane auth (`aut_` only, uniform 401); session open refusals of SEC-94 (deleted key, fingerprint mismatch, foreign CA, removed previous CA key, lost access, frozen, BYO, over each cap) and success with one `agent.ssh.open` row; grant single-use, 30 s TTL, stale boot, closed session; heartbeat revocation per cause closes the row and writes exactly one close audit row; concurrent sweep + terminator close → one transition; wake demand on a parked sandbox, shared budget exhausted → 429 `wake_limited`; C-793 authority (own, `can_own`, other 404).
2. Runner ITs: `auz_` opens an exec only with `kind: "ssh"`; `aux_`/`auy_` never open an exec and `auz_` never a port or a PTY-terminal; missing first frame → 4504; bad frames → 4502; channel cap → 4409; W6 reconnect → 4503; older supervisor (ignores `exec.open`) → 4502 pre-101; keep-awake stamps at most once per grace.
3. Supervisor tests: shell/exec/sftp, with and without tty; stdout vs stderr tags; stdin EOF; `exit 7` → status 7, `kill -9 $$` → signal `KILL`; env probe (no `AURORA_*`, no boot doc, `LANG` passed, `PATH`/`AURORA_X` dropped); `exec.refused` for an unknown mode, a `command` on `sftp`, a missing `sftp-server`; fd-leak probe (FIX-059 style); credit bounds under a hostile writer.
4. Terminator tests (in-process `russh` client, stub API): SEC-92 matrix with byte-identical refusals and the fixed delay; `none` probe uncounted; pre-auth caps, bucket, timeout; every refused channel type/request of `16 §W20` answered as specified with no grant requested; heartbeat failures ×3 → all sessions closed and new auth refused; idle/lifetime with shortened limits; notices printed before disconnect.
5. Helm: `helm template` with `ssh.enabled` and empty `networkPolicy.sshFrom` fails; with it set renders the workload, Services, PDB, policies; `chart-ops`, `edge-routes`, `sandbox-network`, `helm-values-docs` guards pass; `docs/23` rows real.
6. kind e2e `scripts/e2e/ssh.sh` PASSES: key registered, `aurora ssh <agent> -- echo aurora-ssh` and plain `ssh` with a downloaded certificate print it; `exit 7` → 7; `sftp` put/get and `scp` (default and `-O`) of a 5 MiB file with matching sha256; `-L`, `-R`, `-A`, `-X` refused while the shell works; a wrong pinned host key refused by the client; parked sandbox wakes on connect; freeze closes the session within 35 s; secret-grep over the run (canary typed in a shell and passed as a command) finds nothing in API/terminator/runner/supervisor logs, audit or request log.
7. Web: vitest for the sessions lists and disconnect; Playwright + axe both themes, 390 px.
8. `bash scripts/guards/run-all.sh` passes; every `SSH_*` `doc-only` line removed from `limits-gaps.allow` as its constant lands.

## Verification
Per-task gates of `docs/tasks/LANE_RULES.md`; batch gate at the end of each part: `cargo nextest run -p aurora-api -p aurora-runner -p aurora-supervisor -p aurora-protocol -p aurora-terminator` (full targets), `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `bash scripts/guards/run-all.sh`, web lint/test/build, `scripts/e2e/kind.sh` (terminal and previews still pass) and `scripts/e2e/ssh.sh`.

## Dependencies
- Blocked by: P4-SSH-1 merged (keys, CA, `can_ssh`, `GET /v1/ssh`).
- Unblocks: AUDIT of phase 5 SSH items.

## Suggested agent/model and reason
`rust-developer` (Sonnet) for Parts A–D in order A → B → C → D (protocol types first, the terminator last against real runner legs), `nextjs-developer` (Sonnet) for E; Opus reviews Part C's auth state machine and Part D's policies before merge. Size L (a new internet-facing service).

## Completion report
- Files changed:
- Tests/checks:
- Remaining risks:
