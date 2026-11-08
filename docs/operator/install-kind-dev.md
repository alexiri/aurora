# Dev stack on kind (Helm)

(phase 1, OPS-017; replaces the podman compose install.) A local Kubernetes cluster running the
production Helm chart, for development and integration testing. It is **not** a production install: the
Postgres is the chart's evaluation StatefulSet, sign-up is open, sign-in goes through a development identity provider (no passwords) and the images are built from your checkout.
Why this replaced compose, with measurements: [`docs/ops/dev-environments.md`](../ops/dev-environments.md);
design: [docs/15 §Development](../15-deployment-operations.md#development-kind--helm-deploykind-deploydev).
Production installs: [install-helm.md](install-helm.md).

## Prerequisites

`podman` (rootless), `kind` ≥ 0.30, `kubectl`, `helm` 3, `just`, about 15 GB free disk. kind uses the
podman provider (`KIND_EXPERIMENTAL_PROVIDER=podman`, set by the script). No compose provider is needed.

## Commands

```sh
just kind-up                                  # cluster aurora-dev + ingress-nginx + images + helm release
just kind-up --llm-mock --connector-fake      # with the optional profiles (below)
just kind-dev [api gateway hookhost runner connector-fake web sandbox]
                                              # rebuild only what changed, load, restart those Deployments
just kind-status
just kind-down                                # delete the cluster (the cargo cache volume is kept)
```

(`just` forwards to `bash deploy/dev/kind-dev.sh up|dev|status|down`.) Cluster `aurora-dev`, release `aurora` in
namespace `aurora`, sandboxes in `aurora-sandboxes`. **One URL: http://localhost:4000** (the chart's Ingress behind
ingress-nginx, published on 127.0.0.1:4000 by `deploy/kind/kind-config-dev.yaml`). Sign-up is open
(`deploy/kind/values-dev.yaml`). All sign-in is OIDC (phase 1, RFE-004): click **Development sign-in** and pick a user;
your first sign-in creates the account. `kubectl --context kind-aurora-dev -n aurora …` reaches everything.

**First instance administrator.** Pre-provision the e-mail, then sign in with it (the first OIDC sign-in binds the identity):

```sh
deploy/dev/kind-dev.sh bootstrap-admin you@example.test
# = kubectl --context kind-aurora-dev -n aurora exec deploy/aurora-api -- /usr/local/bin/aurora-service bootstrap-admin --email you@example.test
```

The dev images name the binary `/usr/local/bin/aurora-service` (`deploy/dev/thin.Containerfile`), **not**
`/usr/local/bin/aurora-api` as in the release images ([install-helm.md](install-helm.md#post-install-checks)).

**Development IdP.** The chart's `devIdp` component (`aurora-dev-idp`, served at `http://localhost:4000/dev-idp`, reached
by the API in-cluster through `auth.dev_idp.internal_url`) is enabled by `values-dev.yaml` with `allowAnyHint: true`, so a
script or Playwright can sign in any e-mail with `login_hint`; the login page lists `devIdp.users`. It is refused on any
non-loopback `externalUrl`, by the chart and by the API. `just kind-dev dev-idp` rebuilds it.

`scripts/e2e/kind.sh` is separate: it uses its own cluster `aurora-e2e` and is the Kubernetes e2e.

## Fast iteration

`kind-dev` builds the Rust services in a builder container (`deploy/dev/builder.Containerfile`, same Rust image and
linker as the production Containerfile) against the bind-mounted source. Its cargo target and registry live in the
volumes `<worktree>-kind-dev-target` / `-cargo-registry` and `-rustup`, so only the changed crates are recompiled (debug profile,
`--locked`). Each binary is copied into a thin distroless image (`deploy/dev/thin.Containerfile`), loaded with
`kind load image-archive` and its Deployment restarted. A binary identical to the one last loaded is skipped, so
`just kind-dev` after an edit to one service touches only that service. `web` is rebuilt with
`images/services/web.Containerfile` when anything under `web/` changed; for UI work use `just dev` (HMR) instead.
`sandbox` rebuilds `sandbox-min` (fake harness); new sandbox Pods pick it up, no restart.

Host-built binaries are not used: the host glibc is newer than the distroless base, so they would not start.

## Profiles (Helm values)

| Flag | Values file | Adds |
|------|-------------|------|
| (always) | `deploy/kind/values-dev.yaml` | single replicas, open sign-up, the development IdP (`devIdp`), hosted agents with the fake harness (`hosted.allow_fake_harness`), Ingress `nginx` |
| `--llm-mock` | `deploy/kind/values-llm-mock.yaml` + `deploy/dev/llm-mock/llm-mock.yaml` | LiteLLM in front of VidaiMock in namespace `aurora-dev-mock` (decision I-30; **not a model**), endpoint `litellm-mock`, dev key `sk-aurora-mock-dev` ([llm-endpoints.md](llm-endpoints.md#running-with-the-mock-dev-and-e2e-only)) |
| `--connector-fake` | `deploy/kind/values-connector-fake.yaml` | the fake connector Deployment; the API upserts connector `fake` from the generated token (still needs manifest approval, [plugins-and-connectors.md](plugins-and-connectors.md#connectors)) |

The mock's Service has the fixed ClusterIP `10.96.100.40`; the profile opens exactly that `/32` in
`gateway.allowed_private` and gives the gateway's NetworkPolicy one extra egress rule
(`networkPolicy.gatewayExtraEgress`). Nothing else in the cluster is reachable by agents.

Hosted agents: without `--llm-mock` a chat is answered by the fake harness inside a sandbox Pod (this is what the
`k8s.rs` scenarios drive). With `--llm-mock` (P2-E2E-1, decision D-88) the agents run the **real `claude-code`
harness**: `kind-dev.sh` builds `scripts/e2e/llm.Containerfile` on top of the dev `aurora-sandbox` image as
`localhost/aurora-dev/aurora-sandbox-llm:dev` (the Claude Code CLI at the pinned version, installed with npm), loads
it, and `values-llm-mock.yaml` sets `config.runner.image` to it. `just kind-dev sandbox` rebuilds both images.
The default agent needs nothing else: its harness is the default (`claude-code`) and its model is the endpoint's
`default_model` (`scn-pong`, VidaiMock answers `pong`). `values-dev.yaml` sets no gateway address (FIX-010, D-87): sandboxes
dial `gateway.sandbox_proxy_url`, which defaults on the Kubernetes backend to `https://aurora-gateway:4102`, the
name the runner aliases to the gateway proxy inside sandboxes (they have no DNS), while BYO agents keep the external
`gateway.advertised_proxy_url` derived from `externalUrl`.

A second cluster beside another one (or beside the owner's `aurora-dev`): `AURORA_DEV_CLUSTER=<name>
AURORA_DEV_PORT=<free port>` for `kind-up`, `kind-dev`, `first-run.sh` and `m1.sh --kind` (the port is written into a
generated kind config and into `externalUrl`).

## Tests against the dev stack

```sh
bash scripts/e2e/m1.sh --kind        # Playwright M1 flow against http://localhost:4000 (default target: host processes)
bash scripts/e2e/first-run.sh        # P2-E2E-1, needs `just kind-up --llm-mock`; `--up` brings the stack up first
                                     # (sign in, add the LiteLLM key, grant it, say hello, "pong" back; nightly in CI)
AURORA_TEST_KIND=1 AURORA_KIND_API=http://localhost:4000 AURORA_KIND_ORIGIN=http://localhost:4000 \
AURORA_KIND_CONTEXT=kind-aurora-dev AURORA_KIND_NS=aurora AURORA_KIND_SBX_NS=aurora-sandboxes AURORA_KIND_FULL=aurora \
  cargo test -p aurora-e2e-hosted --test k8s -- --test-threads=1     # the Kubernetes scenarios (chat in a sandbox Pod, park/wake, …)
```

`m1.sh --kind` signs two users in through the dev IdP; the API rate-limits OIDC `start`/`callback` per client IP (20/min), so after several runs restart the API
(`kubectl --context kind-aurora-dev -n aurora rollout restart deploy/aurora-api`). The first `kind-up` is slow (about 24 min:
builder image, a cold cargo build, image pulls); later ones reuse the cargo volume and images.

## Inner loop on the host (`just dev`)

`just dev` runs the loop without containers, in [docs/15 §Development](../15-deployment-operations.md#development-kind--helm-deploykind-deploydev):

1. `just dev-db` starts Postgres 16 in podman on `127.0.0.1:55432`, and `just migrate` applies migrations.
2. It sources `deploy/dev/env.sh` (throwaway secrets, `AURORA_CONFIG=deploy/dev/aurora.dev.toml`). Development only.
3. On the first run it mints a dev gateway CA into `deploy/dev/ca/` (gitignored).
4. It runs the API (4101 and 4111), the gateway (4102 and 4103), the hook host (4104) and the web app
   (`pnpm --dir web dev`, port 4100) as host processes. Ctrl-C stops them all.

The web app needs `pnpm install` in `web/` once. The runner is not started; to run hosted agents on the host,
start `aurora-runner serve` with the podman backend ([runners.md](runners.md#podman-backend)).

## Troubleshooting and cleanup

- `kubectl --context kind-aurora-dev -n aurora get pods`; logs: `kubectl … logs deploy/aurora-api`.
- Pods stuck in `ErrImageNeverPull`: the image was not loaded; run `just kind-dev <svc>` (after `kind-down` the stamps are reset).
- Port 4000 busy: another process holds it; stop it before `kind-up` (the port is part of the cluster config).
- Remove everything: `just kind-down`, then `podman volume rm <worktree>-kind-dev-target <worktree>-kind-dev-cargo-registry <worktree>-kind-dev-rustup` and `-rustup`.
