# Dev environments: kind + Helm vs podman compose (OPS-017)

Question (owner, 2026-11-08): "do we really need a podman compose dev option if we can run the kubernetes
setup with kind? can we maintain just the kubernetes setup and use that for local testing/dev?"
**Answer: yes. Compose is removed (decision I-42); kind + the Helm chart is the only containerized setup.**
Host-process `just dev` stays for the inner loop. No blocker was found: CI never used compose
(`.github/workflows/ci.yml` has no reference), and every script or doc that did is migrated (list at the end).

Measured on the dev host (16 CPUs, 31 GB RAM, rootless podman 6.1, kind 0.33, Kubernetes 1.37, 2026-11-07).
"Compose" numbers come from the pre-removal files run from a scratch copy.

## Measurements

| | compose | kind + Helm |
|---|---|---|
| Cold start, everything built from scratch (Rust builder + images + cluster + install) | not measured separately: the same release `Containerfile` builds the images (the `scripts/e2e/kind.sh --fresh` baseline of 16 min in `docs/audit/phase0-exit.md` is dominated by it) | `just kind-up --llm-mock --connector-fake`: **24 min** (cluster create + ingress-nginx ≈ 1.5 min; builder image incl. pulling the Rust image ≈ 5 min; cold cargo build of five services in the builder container 9 min 28 s; web and `sandbox-min` image builds and loads the rest, including a first pull of LiteLLM) |
| Start with images already present | `podman compose up -d` → API answering through the edge: **19 s** | cluster create + ingress + `helm install --wait` (images loaded): ≈ 3 min (cluster ≈ 70 s, install with Postgres, migrate hook, secrets/CA hooks ≈ 70 s) |
| Warm iteration: change one service and roll it | rebuild the release image with the production `Containerfile` and recreate the container: **24 min** measured here, but with a cold dependency layer on this host; the production `Containerfile` recompiles every workspace crate in release mode (thin LTO) on any source change, so a warm number is minutes, never seconds | `just kind-dev hookhost`: **33 s**; `just kind-dev api`: **42 s** (incremental compile in the builder container + thin image + `kind load` + rollout) |
| Resident memory, idle | api, gateway, hookhost, web, postgres, edge (no runner, no LLM mock): ≈ 240 MB in total | whole stack incl. runner, LiteLLM, VidaiMock, ingress-nginx, kindnet, API server and etcd: **2.1 GB** for the node container |
| Disk | images only | images + node image (~1 GB) + builder image (1.85 GB) + the cargo target volume (debug build of five services, several GB) |

Reading: compose is lighter and starts faster when images exist; kind costs about 2 GB of RAM and a few
minutes more at cold start. After the first build, the day-to-day loop on kind (30–40 s) is much shorter
than rebuilding release images, which is what compose needed (it could not use the incremental builder).
For UI and API inner-loop work neither is used: `just dev` (HMR, `cargo run`) stays the fast path.

## What compose gave that kind did not (and the replacement)

| Compose feature | Replacement on kind | Status |
|---|---|---|
| `edge`/Caddy on `localhost:4000` (one URL) | chart Ingress + ingress-nginx, node `hostPort 80` → `127.0.0.1:4000` (`deploy/kind/kind-config-dev.yaml`, `ingress-nginx.yaml`) | done; `m1.sh --kind` signs up, invites and accepts through it |
| `llm-mock` profile (LiteLLM → VidaiMock, I-30) | `deploy/dev/llm-mock/llm-mock.yaml` + `deploy/kind/values-llm-mock.yaml` (fixed ClusterIP `10.96.100.40`, `gateway.allowed_private`, new `networkPolicy.gatewayExtraEgress`) | done; the scenario smoke (`llm-smoke.sh`) passes against it in-cluster and a gateway-labelled pod reaches it under the NetworkPolicies |
| `connector-fake` profile | `deploy/kind/values-connector-fake.yaml` (chart `connectors` entry, bootstrap token from the generated Secret) | done; the API logs `bootstrap connector token applied` for `fake` |
| `hosted` profile | not needed as a profile: the kind runner uses the Kubernetes backend, so hosted agents are on by default | better than compose: compose's runner container could not open the rootless podman socket without `keep-id` (phase0-exit Finding 1), kind runs sandboxes as Pods under NetworkPolicies |
| `dev-idp` profile (RFE-004) | none; the compose profile never existed in the repository | to be added by RFE-004 as a chart/kind component |
| single-host operator install | the Helm chart on any small cluster (k3s) | the compose install doc is deleted; operators use `install-helm.md` |

Not covered: a hosted `claude-code` agent answering through the mock inside kind needs a sandbox image with
the Claude Code CLI (`scripts/e2e/llm.Containerfile`), which the dev flow does not build by default.
`scripts/e2e/llm.sh` still proves that path with its own podman containers.

## Developer story

- **Compose (before):** `just compose-up` (secrets in `.env`, CA minted into a volume, static IPs on two
  networks); images built from the release `Containerfile` (no incremental compile); different networking and
  runner backend from production, so SEC-02/09 and NetworkPolicy behaviour were only tested on the CI kind job.
- **kind (now):** `just kind-up [--llm-mock] [--connector-fake]`, then `just kind-dev <svc>` per change. The same chart,
  Ingress, hooks and Kubernetes runner backend as production; `kubectl` for logs. `deploy/kind/values-dev.yaml` is
  the only dev-specific configuration. Gotchas found and handled: ingress-nginx `worker-processes: auto` spawns
  16 workers and fails `pthread_create` in the rootless node (set to 2); Kubernetes injects `VIDAIMOCK_PORT`
  service-link variables that VidaiMock reads as its port (`enableServiceLinks: false`); the API rate-limits
  sign-ups per client IP, so repeated `m1.sh --kind` runs need `kubectl rollout restart deploy/aurora-api`.

## Fast image iteration

Host-built binaries copied into a thin image were considered and rejected: the host glibc (2.44) is newer than the
distroless base (Debian 12, 2.36), so they do not start. Instead `deploy/dev/kind-dev.sh` builds in a builder
container (Rust image and lld as in the production `Containerfile`) with the source bind-mounted and the cargo
target in a named volume, copies the five binaries into `distroless/cc` thin images and only loads and restarts
services whose binary changed. It must build the same package set every time: building a subset unifies cargo
features differently and recompiled the whole dependency graph (observed). Coordination with OPS-018 (live
reload): if Tilt is adopted there, `kind-dev` is its fallback; the thin-image build is reusable as a Tilt
`docker_build` step.

## What was migrated

`deploy/compose/` (compose.yaml, Caddyfile, aurora.toml, setup.sh, .env.example) deleted; VidaiMock and LiteLLM
config moved to `deploy/dev/llm-mock/`; `scripts/e2e/llm.sh` reads the new paths; `just compose-up|down`
replaced by `just kind-up|kind-dev|kind-status|kind-down`; `scripts/guards/edge-routes.sh` checks only the Helm
Ingress and HTTPRoute (also rendered with the kind dev profile); `.gitignore`/`.dockerignore` entries dropped;
`docs/operator/install-compose.md` deleted and `install-kind-dev.md` added; operator pages (README, backups,
upgrades, key-rotation, runners, llm-endpoints, plugins-and-connectors, install-helm), docs 00, 01, 02, 15, 18, 19
and decision I-42 updated. Left as history on purpose: `docs/audit/phase0-exit.md`, `docs/tasks/*`, PROGRESS and
FEATURES. Not touched (owned elsewhere): `docs/04` (the `dev-idp` profile), `docs/06` and `docs/11` mention the
compose addresses, and `services/aurora-gateway/src/proxy_tls.rs` still adds the fixed compose gateway address
(`10.89.250.2`) to the proxy certificate when it is local; harmless, and removable in a code change.
