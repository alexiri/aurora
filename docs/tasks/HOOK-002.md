# HOOK-002: `aurora-hookhost` service (wasmtime runtime, W9 endpoint, limits, egress-filtered fetch, snapshot pull)

## Linked requirements
- M3/09 `§8.3 The hook host` (every bullet), `§8.2` (outputs validated against the manifest: declared credential/metadata keys, tenant-host in zone, `continue.url` https on declared/authorize host, summaries clamped), `16 §W9`, `02 §Limits` (`HOOK_MEMORY` 64 MiB, `HOOK_OUTPUT` 256 KiB, `HOOK_WALL` 15 s / 1 s, `HOOK_FETCH_MAX` 16 calls × 1 MiB × 10 s), I-20 (internal-secret bearer), D-23 (pulls snapshot/modules from API 4111; no DB, no keys); SEC-13, SEC-14; `18 §Plugins` (hostile module that loops, allocates and floods output).

## Scope (`services/aurora-hookhost/**`)
- Bindings from `aurora-hook-wit` via `wasmtime::component::bindgen!`; engine with epoch interruption + memory limiter (ResourceLimiter); module cache by digest; fresh Store per call; host impl of `fetch` (https only; host ∈ package egress list for this hook from the snapshot; destination guard: no private/loopback/link-local/metadata; no redirects; caps), `log` (redacted, capped), `random-bytes`, `now-unix-seconds`.
- `POST /internal/v1/hooks/{hook}` per W9; snapshot/blob pull from `hookhost.api_internal_url` every 60 s + on miss; readyz = snapshot loaded; metrics per package/hook.
- Output validation per `§8.2` using the snapshot app definitions; errors mapped to `{error:{code,message}}` with codes `unsupported`, `invalid_input`, `upstream`, `reauth_required`, `internal`, `timeout`, `memory`, `output_too_large`, `egress_denied`.

## Acceptance
- [ ] Integration tests with guest components built in-test (`cargo build --target wasm32-wasip2` of small guests under `services/aurora-hookhost/tests/guests/`): happy path for each hook; egress denied for undeclared host and for any fetch from `approval.summarize`; infinite loop killed by epoch (1 s for summarize); allocation bomb killed by memory limit; output flood rejected; invalid output keys rejected; auth required; snapshot pulled from a mock API internal endpoint.
- [ ] `cargo test -p aurora-hookhost`, clippy, fmt, guards (wasmtime only in hookhost/registry dev paths).

## Dependencies: HOOK-001, BE-041 (internal routes; mockable). Agent: rust-developer (Sonnet).

## Hand-off from HOOK-001 (must do)
- Decision I-22: link the allowed WASI 0.2 interfaces with empty sandboxes (wasmtime-wasi `WasiCtxBuilder` with no env/args/preopens, stdin closed, stdout/stderr captured into the capped redacted log); never link filesystem/sockets.
- `wasmtime = { version = "38", default-features = false }` is pinned in root deps — enable the features you need in hookhost's Cargo.toml (`component-model`, `cranelift`, `runtime`, plus `wasmtime-wasi` matching version). Guests for tests: follow `crates/aurora-registry/tests/guest_components.rs` (standalone cargo projects built with `--target wasm32-wasip2` into `target/hook-guests`); SDK: `aurora-plugin-sdk` (`Hooks` trait + `export_hooks!`).
