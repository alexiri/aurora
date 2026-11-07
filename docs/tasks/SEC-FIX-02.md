# SEC-FIX-02: Gateway internal (4103) and runner ops (4106) listeners must not be reachable from sandboxes (SEC-09)

## Finding (TEST-013 s8b)
- Gateway 4103 and runner 4106 bind the same address as the sandbox-facing listeners (4102 / 4105), so a sandbox reaches them (health/metrics unauthenticated; internal routes behind the internal secret only). `01 §Trust boundaries` 5, `06 §Listeners`, SEC-09: internal ports are never on the sandbox network.

## Required
- Config: `gateway.internal_bind` (address for 4103) and `runner.ops_bind` (address for 4106), separate from the proxy/channel bind (`server.bind`); validated at startup. Document defaults (`0.0.0.0` is acceptable only where NetworkPolicy isolates — Helm), and make compose bind them to the **core-network** address: give gateway and runner static core-network IPs in `deploy/compose/compose.yaml` and set the binds via env; update `deploy/compose/aurora.toml`, Helm ConfigMap values (keep 0.0.0.0 + NetworkPolicy), `deploy/dev/aurora.dev.toml` (127.0.0.1).
- Defence in depth: if the gateway's internal listener and proxy listener resolve to the same IP (or the internal bind is unspecified on a host where the proxy bind is also unspecified) log a startup **warning** naming SEC-09; same for the runner.
- Tests: config validation; a unit/integration test that binds the two listeners to different loopback addresses (127.0.0.2 vs 127.0.0.3) and asserts each listener is only on its address. Update `tests/e2e-hosted` common harness config to bind 4103/4106 on a non-sandbox address so `s8b` passes (coordinate: the TEST-013 agent may be running — only touch the config-building helper there and say what you changed).

## Scope: `crates/aurora-config`, `services/aurora-gateway/src/main.rs` (+ listener setup), `services/aurora-runner/src/main.rs` (+ ops listener), `deploy/compose/**`, `deploy/dev/**`, `deploy/helm/aurora/values.yaml` + configmap template, `tests/e2e-hosted/tests/common/mod.rs` (config helper only). Checks: `cargo test -p aurora-config -p aurora-gateway -p aurora-runner`, clippy, fmt, guards, `helm lint`. Agent: rust-developer (Sonnet). Priority: **security, high**.
