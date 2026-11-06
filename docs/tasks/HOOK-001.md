# HOOK-001: `aurora-hook-wit` (WIT world) + `aurora-plugin-sdk` (guest) + validator component checks

## Linked requirements
- M3/09 `§8.1 WIT world` (normative WIT text — copy verbatim into `crates/aurora-hook-wit/wit/plugin.wit`), `§8.2` (hook points; authors implement only what `permissions.hooks` declares; SDK defaults return `unsupported`), `§12` (`aurora-plugin-sdk`: generated bindings, `#[aurora_plugin::hooks]`-style ergonomics, `fetch`/`log` helpers, test utilities), `§6` validator rule "`hooks.wasm` exports/imports not matching the WIT world" (component importing anything other than `aurora:plugin/host` is rejected), I-02a (target available).

## Scope
- `crates/aurora-hook-wit/`: the WIT file + a tiny lib exposing `WIT_DIR`/`WORLD` consts (host side generates bindings in the hook host with `wasmtime::component::bindgen!`).
- `crates/aurora-plugin-sdk/`: guest crate using `wit-bindgen` (add to root `[workspace.dependencies]`; choose a version compatible with the wasmtime version you also add for later use — record both); a `Hooks` trait with default `unsupported` methods + an `export_hooks!(MyType)` macro (a proc-macro attribute is optional — a declarative macro is fine; document); `fetch`/`log` wrappers; a `testing` module (sample inputs). Must compile for `wasm32-wasip2` and `cargo check` on the host target.
- `crates/aurora-registry`: replace REG-002's `hooks_wasm_invalid` placeholder with a real check using `wasmparser` (component validation: it is a component; imports ⊆ `aurora:plugin/host@1.0.0`; exports include `aurora:plugin/hooks@1.0.0`); export names used ⇔ `permissions.hooks` (best effort: if export function presence can't distinguish implemented vs default, check only the world shape and document).
- An example guest under `crates/aurora-plugin-sdk/examples/` or `tests/guest/` built in a test via `cargo build --target wasm32-wasip2` (skip with a clear message if the target is missing) and fed to the validator.

## Acceptance
- [ ] `cargo build -p aurora-plugin-sdk --target wasm32-wasip2`; example guest builds to a component; validator accepts it and rejects a component importing `wasi:filesystem` (build a second tiny guest that uses `std::fs`).
- [ ] `cargo test -p aurora-registry -p aurora-hook-wit -p aurora-plugin-sdk`, clippy, fmt, guards (dep-graph: api/gateway still free of wasmtime).

## Dependencies: REG-002, I-02a. Unblocks: HOOK-002, HOOK-003. Agent: rust-developer (Sonnet).
