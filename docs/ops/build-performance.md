# Build performance

(OPS-016 owns the local-build sections; this file currently holds only the CI section from OPS-019.)

## CI (OPS-019)

Layout of `.github/workflows/ci.yml`:

| Job | Purpose |
|---|---|
| `rust-lint` | fmt + clippy (own cache `rust-lint`, own profile) |
| `rust-deny` | cargo-deny |
| `rust-build` | compiles once: fixture packages, `cargo nextest archive`, doctests; uploads the archive |
| `rust-test` (×4) | `cargo nextest run --archive-file … --partition count:i/4`, no compile |
| `web` | pnpm lint/typecheck/test/build/API drift |
| `guards` | `scripts/guards/run-all.sh`, no compile |

Changes and why:
- All actions pinned by commit SHA, all on Node 24 (`runs.using` verified per action; `cargo-deny-action` is Docker, `install-action`/`rust-toolchain` are composite). `ubuntu-24.04` pinned instead of `ubuntu-latest`.
- sqlx-cli binary cached with `actions/cache` (was `cargo install` in three jobs); nextest via `taiki-e/install-action`.
- lld via `RUSTFLAGS=-C link-arg=-fuse-ld=lld`, `CARGO_PROFILE_DEV_DEBUG=0` (disk). Align with OPS-016's `.cargo/config.toml` once merged.
- `rust-cache` is saved by `rust-build` only (key `rust-test`), `rust-lint` has its own key.
- **Coverage fix:** `connect.rs` (12 tests) failed in CI because `plugins/fixtures/dist` is gitignored and never built; `rust-build` now runs `scripts/fixtures/build.sh` and ships the packages to the shards.
- `aurora-e2e-gateway`/`aurora-e2e-hosted` are excluded: they are not env-gated, need fixed IPs/ports, podman and serial runs (`scripts/e2e/*.sh`); they fail under plain `cargo test`.
- `schema_matches_spec` now runs in the shards instead of the `guards` job (same test, same migrated DB).

### Timings

| | rust-test (cold) | rust-test (warm) |
|---|---|---|
| Before (run 42367335980) | 13m47s, failed (linker SIGBUS) | n/a |
| After | TO BE RECORDED after the first push | TO BE RECORDED |

Not measured locally: nextest could not be installed in the worker sandbox.
