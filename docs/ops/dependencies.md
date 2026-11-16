# Dependency updates (OPS-021)

Dependabot (`.github/dependabot.yml`) opens update PRs for every ecosystem in the repository.
This page is the review policy for those PRs.

## What Dependabot watches

| Ecosystem | Directory | Weekly slot (Europe/Zurich) | PR shape |
|---|---|---|---|
| `cargo` | `/` (the root workspace, `Cargo.lock`) | Mon 06:00 | one grouped PR for minor + patch; each major its own PR |
| `cargo` | `plugins/builtin/{aws-keys,github,google-workspace}/hooks` (own lockfiles, D-77) | Mon 06:30 | grouped minor + patch |
| `npm` (pnpm) | `web/` (`pnpm-lock.yaml`) | Tue 06:00 | grouped minor + patch; majors separate |
| `npm` | `images/sandbox/harness/claude-code` | Tue 06:30 | nothing: the CLI is ignored (below) |
| `github-actions` | `/` (workflow actions pinned by commit SHA) | Wed 06:00 | grouped minor + patch |
| `docker` | `images/sandbox`, `images/services`, `deploy/dev/llm-mock`, `deploy/kind` | Wed 06:30 | grouped minor + patch |

**Security updates** are raised as soon as GitHub publishes the advisory, one PR per package, outside
the weekly slot and the groups. Treat them as urgent: review and merge the same day when CI is green.
Labels: `dependencies` plus `rust`, `javascript`, `github-actions` or `docker`. Commit titles start with
`deps(<area>):`. Open PRs are capped per ecosystem (`open-pull-requests-limit`), so an unattended
backlog stops growing instead of burying CI.

## Pinned on purpose (ignored by Dependabot)

| Item | Why | How it is updated instead |
|---|---|---|
| `wasmtime`, `wasmtime-wasi` (majors only) | The hook host's component-model API, epoch interruption and limits change between majors (HOOK-001/002). Minor and patch releases still arrive. | By hand, with `aurora-hookhost`'s tests and the plugin hook conformance cases. |
| `wasmparser`, `wit-bindgen` (majors and minors; root workspace and hook crates) | The component-model tooling of the same wasmtime line: the registry validates hook components with `wasmparser` before wasmtime loads them, and the guests are built with `wit-bindgen`. As 0.x crates they break their API on minors (PR #13). Patch releases still arrive. | By hand, together with a wasmtime bump: `aurora-registry` and `aurora-hookhost` tests, the hook crates rebuilt and re-signed, the plugin hook conformance cases. |
| `sqlx` (majors and minors) | sqlx 0.9 accepts only `&'static str` queries; every run-time SQL string must be audited and wrapped in `AssertSqlSafe` (PR #29: 305 errors in `aurora-db` alone), and it changes migration ordering. Patch releases of 0.8 still arrive. | By hand in FIX-160, at a quiet point between lane chains; the entry is removed in that merge. |
| `@types/node` (majors only) | The web runtime is Node 22 (web image, CI) with a documented dev floor of 20.9; the types must track the runtime major, not run ahead of it (26.x was proposed). Minor and patch releases still arrive. | By hand, together with the Node runtime (web `engines`, `web.Containerfile`, CI `setup-node`, README). |
| `jsdom` (majors only) | jsdom 31 needs Node `^22.22.2 \|\| ^24.15.0 \|\| >=26`, above the 20.9 floor in `web/package.json`. | By hand, together with raising the Node floor (engines, README) and a full vitest run. |
| `@anthropic-ai/claude-code` | The version must equal `PINNED_VERSION` in `services/aurora-supervisor/src/adapters/claude_code/mod.rs` (FIX-103). | By hand, together with the supervisor constant and the claude-code conformance run. |
| LiteLLM and VidaiMock digests (`ghcr.io/berriai/litellm`, `ghcr.io/vidaiuk/vidaimock`) | I-30: a changed mock changes what the live claude-code acceptance proves. | By hand, with `scripts/e2e/llm.sh` and the hosted e2e. |
| Mattermost 11.7 (`mattermost/mattermost-preview`) | MM-003: 11.7 is the supported production version, the dev image is pinned to 11.7.15 by digest. | By hand, with `scripts/conformance.sh --mattermost` and `scripts/e2e/mattermost.sh`. |

Each is an `ignore` entry with its reason in `dependabot.yml`; change the entry in the same PR that
changes the pin.

## Container images

Dependabot's `docker` ecosystem parses files named `Dockerfile*` and Kubernetes manifests. The
repository's image recipes are named `Containerfile`, so **their base-image digests
(`SANDBOX_BASE_IMAGE`, `RUST_IMAGE`, `DISTROLESS_IMAGE`) are not updated by Dependabot**; the `docker`
entries cover the Kubernetes manifests under `deploy/` (and any `Dockerfile` that appears). Until the
recipes are renamed, refresh those digests monthly by hand, check them against the base image's release
notes, and run `bash scripts/guards/run-all.sh` plus an image build. This is a known gap, not an oversight.

## Review policy for a dependency PR

1. **CI must be green** on the PR. That is the whole `ci` workflow: `rust-deny`, build + test shards,
   clippy, the web job (lint, typecheck, vitest, build, Playwright mocked and production), `guards`
   and the image builds. A red check is never waved through; a flaky one is re-run once, and a second
   failure is investigated.
2. **`cargo deny` gates Rust PRs** (`rust-deny` runs `deny.toml` on the root workspace and
   `scripts/plugins/deny-hooks.sh` on each hook crate): licenses outside the allow-list, new advisories
   and yanked crates fail the PR. A waiver is added to `deny.toml` only with a reason and a tracking
   note, in its own reviewed commit; never to make a Dependabot PR pass.
3. **Read the diff of the lockfile**, not just the manifest: new transitive crates or packages are new
   code we ship. For a grouped PR skim the release notes of every major-looking line; for a major
   update read the changelog and the migration guide first.
4. **Majors, and anything touching security-sensitive crates** (crypto, TLS, OIDC, SQL, the sandbox and
   hook runtime) get a human review of behaviour, not only CI. Run the relevant extra gate locally:
   `aurora-api` it-suite for database or HTTP-stack crates, the e2e script for the area
   (`gateway.sh`, `channels.sh`, `hosted.sh`), the web Playwright projects for UI libraries.
5. **Do not widen the PR.** If an update needs code changes, push them to the Dependabot branch as a
   separate commit, or close the PR and do the bump by hand under a task id.
6. **Who merges.** The repository owner (or the coordinator session acting for them) merges dependency
   PRs; workers do not merge. Grouped weekly PRs are reviewed within the week; security PRs the same
   day. A PR that cannot be merged within two weeks is closed with a comment saying why (and an
   `ignore` entry if the pin is intentional) so the bot does not reopen it.

## Known Dependabot quirks

- **Web PRs drop the `overrides:` block of `web/pnpm-lock.yaml`.** `web/package.json` has
  `pnpm.overrides` (minimatch), and Dependabot's regenerated lockfile omits it, so the `web` job dies
  in `pnpm install --frozen-lockfile` with `ERR_PNPM_LOCKFILE_CONFIG_MISMATCH` before any test runs.
  Fix on the Dependabot branch: `pnpm -C web install --lockfile-only`, commit the lockfile.
- **GitHub-side merges of several Cargo PRs can corrupt `Cargo.lock`** (textual merge of adjacent
  entries; seen with PRs #8 + #9: `toml_datetime` listed twice). After merging more than one cargo PR
  in a row, run `cargo metadata --locked` on master. Better: merge one, then `@dependabot rebase` the
  next before merging it.
- **Paired crates come grouped**: `utoipa` + `utoipa-axum` and `kube` + `kube-*` + `k8s-openapi`
  only work in matching versions, so `dependabot.yml` groups each pair (all update types) and one PR
  moves both (PRs #18, #19 bumped one half and could not build).
- **RustCrypto crates move as one line**: `sha2`, `hmac`, `md-5` (and `sha1`, `digest`, `hkdf`) share the
  `digest` major, so `dependabot.yml` groups them (`rustcrypto`); a lone `sha2` 0.11 bump failed to build
  against `hmac` 0.12 (PR #23). Moving to 0.11/0.13 also drops `LowerHex` on digest output (use
  `hex::encode`) and needs `hmac::KeyInit` for `new_from_slice`.
- **`kube` 5 needs Rust 1.95**: the kube group's bump (PR #22) raised the workspace `rust-version` from 1.88
  (I-21) to 1.95; CI and `rust-toolchain.toml` use stable, so only the declared MSRV changed.
- **Docker dependency names drop the registry**: Dependabot reports `ghcr.io/berriai/litellm` as
  `berriai/litellm`, so an `ignore` entry lists both spellings (PR #3 slipped past the first one).

## Checking the configuration

The schema is GitHub's; after editing `.github/dependabot.yml`, the repository's **Insights →
Dependency graph → Dependabot** tab shows parse errors and the last run of each update. The first
weekly run after a change should be checked there: every ecosystem listed above must show a run, and
the cargo, npm, actions and docker updates should arrive grouped as the table says.
