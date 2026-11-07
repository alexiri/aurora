# SEC-FIX-01: Supervisor — scrub every child environment (SEC-34 / I-23), expose local `process_*` tools in tools/list

## Finding (TEST-013)
- `services/aurora-supervisor/src/process.rs` `shell()` (process manager + gates) and the `claude-code` launcher spawn children without `scrub_command`/`scrub_tokio_command`; nothing calls the scrubbers ⇒ children (the harness, background processes, gates, and the MCP bridge spawned by the harness) can inherit `AURORA_BOOT` (boot token = live gateway credential, bootstrap token). SEC-34 violated; I-23 depends on scrubbing.
- Local `process_*` tools are executed by the supervisor but never listed: `tools.rs` `list` answers only from `.aurora/tools.json` (API catalogue).

## Required
- Make env scrubbing **structural**: a single `spawn` helper (std and tokio) that starts from an **allow-list** environment (not a deny-list): the non-secret container env of `11 §The boot document` (TZ, HOME, LANG, PATH, AURORA_SANDBOX_ID, AURORA_AGENT_HANDLE, proxy vars pointing at the loopback proxy, CA vars, app env from the snapshot) plus per-session additions; `AURORA_BOOT` and any `*_TOKEN`/secret never pass. Replace every `Command::new`/`tokio::process::Command::new` in the crate with it; add a CI-style test (grep in a unit test, or a clippy `disallowed_methods` config for this crate) so new spawns can't bypass it.
- Merge local tool descriptors (`process_start/status/stop/watch` with JSON Schemas) into the tools/list answer (and document that the API catalogue contains only remote tools).
- Tests: spawn via the process manager, a gate and the claude-code launcher (with a stub binary) and assert the child's `/proc/self/environ` (or `env` output) lacks `AURORA_BOOT` and contains the expected allow-listed variables; tools/list includes `process_*`.

## Scope: `services/aurora-supervisor/**` only. Checks: `cargo test -p aurora-supervisor`, clippy, fmt, guards. Agent: rust-developer (Sonnet). Priority: **security, highest**.
