# Install with Helm (Kubernetes)

This is the production install described in
[docs/15 §Production](../15-deployment-operations.md#production-helm-deployhelmaurora).
The chart is `deploy/helm/aurora/` (Chart `aurora` 0.1.0, appVersion 0.1.0, `kubeVersion >=1.30.0-0`).
It renders the same `aurora.toml` structure as compose into the ConfigMap `<fullname>-config`.

Names below use the release name `aurora`. The fullname is the release name when it
contains `aurora`, otherwise `<release>-aurora`
(`templates/_helpers.tpl`, `aurora.fullname`).

## Prerequisites

- Kubernetes 1.30 or later.
- A CNI that enforces NetworkPolicy. The chart renders default-deny policies in both
  namespaces, and they do nothing without an enforcing CNI.
- A default StorageClass that supports `ReadWriteOnce`. `allowVolumeExpansion` is recommended.
- An external PostgreSQL 16 database reachable from the cluster. The bundled
  `postgres.enabled` is for evaluation only (see [Database](#database)).
- An Ingress controller (values default to ingress-nginx) or a Gateway API implementation.
- A runtime class for sandboxes, such as gVisor or Kata (recommended by docs/15).
- Images pushed to a registry you control (`image.registry`), or available under bare
  names. Tag defaults to the chart's appVersion (`image.tag`).
- The sandbox image must be pullable by the nodes under the name in `config.runner.image`
  (default `aurora-sandbox:<version>`, which containerd resolves to
  `docker.io/library/aurora-sandbox:<version>`). The runner applies that setting to every
  sandbox Pod and the API reports it as the sandbox's `image`; set it to your mirrored name.

The chart is checked with `helm lint`, `helm template` and an install on a local kind
cluster (see [Test on a local kind cluster](#test-on-a-local-kind-cluster)), which runs the
hosted scenarios and the sandbox isolation probes against the `kubernetes` runner backend.

## Namespaces

- **Release namespace** (for example `aurora`): core services, hooks, ConfigMap, Secrets, NetworkPolicies.
- **Sandboxes namespace**: `sandboxes.namespace`, default `aurora-sandboxes`. Created when
  `sandboxes.create: true`. It has a ResourceQuota `sandboxes`, a LimitRange `sandboxes`,
  Pod Security Admission labels `restricted` (enforce and audit), and a runner Role
  and RoleBinding.

## Secrets

The chart uses one Secret for all generated keys. Choose one of two options.

**Option A: let the chart generate them.** Leave `secrets.existingSecret` empty.
The `<fullname>-secrets` pre-install job (`secrets`, weight -10; it needs `sh`, `kubectl`,
`base64`, `head`, `od` and `tr` in the hook image, not `openssl`) creates
`secrets.generatedName` (default `<fullname>-secrets`) and writes only the missing keys.
It never overwrites a value. The generated keys are:
`AURORA_ENCRYPTION_KEYS`, `AURORA_ENCRYPTION_ACTIVE_KEY`, `AURORA_SIGNING_KEY`,
`AURORA_INTERNAL_SECRET`, `AURORA_RUNNER_SESSION_KEY`, `AURORA_RUNNER_TOKEN`,
`AURORA_CONNECTOR_TOKEN`. With `postgres.enabled: true` it also writes
`POSTGRES_PASSWORD` and `AURORA_DATABASE_URL`.

**Option B: bring your own.** Set `secrets.existingSecret` to a Secret you created.
The generator hook is then not rendered. The Secret must contain every key the
workloads read:

| Key | Read by |
|-----|---------|
| `AURORA_ENCRYPTION_KEYS`, `AURORA_ENCRYPTION_ACTIVE_KEY` | api, gateway, migrate |
| `AURORA_SIGNING_KEY` | api, migrate |
| `AURORA_INTERNAL_SECRET` | api, gateway, hookhost, migrate |
| `AURORA_RUNNER_SESSION_KEY` | api, runner, migrate |
| `AURORA_RUNNER_TOKEN` | runner, and the API bootstrap (`api.bootstrap.runnerToken`, default true) |

`AURORA_CONNECTOR_TOKEN` is generated into the shared Secret, but no chart workload reads it.
Each connector takes its token from its own `tokenSecret` (see [Connectors](#connectors-optional)).

Generate values by the same rules `setup.sh` uses (`docs/15 §Configuration`): keys are
`k1:<base64 32 bytes>`, the signing and internal secrets are base64, and the runner
token is `aur_…`. Generated secrets are unrecoverable without the keyring. See [backups.md](backups.md).

## Database

External PostgreSQL is expected in production.

- Create the Secret named `database.existingSecret`, or `<fullname>-database` when empty,
  with key `AURORA_DATABASE_URL` (`postgres://user:pass@host:5432/aurora`). The chart does
  not create it when `postgres.enabled` is false.
- `database.maxConnections.api` (default 20) and `database.maxConnections.gateway` (default 10)
  become `database.max_connections` for each service.
- `database.gatewayRole` sets `database.gateway_role`. `aurora-api migrate` applies the
  gateway-role grants when it is set (`services/aurora-api/src/main.rs`).
- `database.cidrs` and `database.port` (default 5432) set the egress rule to the database in the NetworkPolicies.
- No code reads `AURORA_GATEWAY_DATABASE_URL`. The gateway uses `AURORA_DATABASE_URL`, and
  `database.gatewayRole` is the least-privilege switch.

`postgres.enabled: true` renders a single-instance Postgres for evaluation. Its StatefulSet,
Service and PVC are pre-install hook resources, so `helm uninstall` leaves them and the data
stays on the PVC (FUP-M7). Hook delete policies cannot cover uninstall, so the chart documents
it instead (`NOTES.txt` prints the cleanup commands). Delete them by hand:

```bash
# Release "aurora" (fullname "aurora"); otherwise the names start with <fullname>-postgres
kubectl -n aurora delete statefulset,service aurora-postgres
kubectl -n aurora delete pvc data-aurora-postgres-0
```

## Gateway CA

The `<fullname>-gateway-ca` pre-install job (`gateway-ca`, weight -9) runs
`aurora-gateway generate-ca --out /ca` and stores `ca.pem` and `ca.key` as the Secret
`<fullname>-gateway-ca`. If the Secret exists, it does nothing ("CA secret exists; not
overwriting"). To use your own CA, create a Secret with keys `ca.pem` and `ca.key` and
set `gateway.ca.existingSecret`. The gateway mounts the whole Secret at `gateway.ca_dir`.
The API mounts only `ca.pem` there, because it embeds the certificate in the boot documents
it hands to sandboxes. It never receives `ca.key`.

## Configuration

`config.*` in `values.yaml` is rendered into `aurora.toml`. The chart sets these keys
itself, so do not override them:

- `server.external_url` from `externalUrl`;
- `runner.backend = "kubernetes"`, `runner.api_url`, and `runner.k8s.namespace`,
  `core_namespace`, `gateway_service`, `runner_service`, plus `storage_class` and
  `runtime_class` when `runner.k8s.storageClass` and `runner.k8s.runtimeClass` are set;
- `database.max_connections`, `database.gateway_role`;
- `gateway.ca_dir = "/var/lib/aurora/ca"`.

Chart defaults that you may set in `config`: `server.bind`, `server.trusted_proxies`
(default private ranges, the edge or ingress pod CIDRs), `gateway.internal_bind` and
`runner.ops_bind` (both `0.0.0.0`, acceptable only because the NetworkPolicies block
sandbox ingress to 4103 and 4106), `hosted.enabled`, `telemetry.log_level`.

Add `auth`, `llm`, `governance`, `secrets`, `email` and the other sections the same way.
The values file says so, and [llm-endpoints.md](llm-endpoints.md) shows an example.
`plugins.dir` is set from the top-level `plugins.dir` value (default `/var/lib/aurora/plugins`),
so do not set it in `config`. `runner.k8s.nodeSelector` and `runner.k8s.tolerations` are
rendered into `[runner.k8s]` as `node_selector` and `tolerations`.

### Plugin packages

The API scans `plugins.dir` at startup and every 60 s (see [plugins-and-connectors.md](plugins-and-connectors.md#plugins)).
`plugins.source` chooses where the packages come from, for the API pods only:

| `plugins.source` | Set | What the chart does |
|------------------|-----|---------------------|
| `none` (default) | nothing | No package volume. Upload packages with the admin API instead. |
| `configMap` | `plugins.configMapName` | Mounts the ConfigMap in an init container (`seed-plugins`), which copies each `*.aurora-plugin` key into an emptyDir at `plugins.dir`. The copy is needed because the scan skips symlinks, and ConfigMap files are symlinks. Packages must fit the 1 MiB ConfigMap limit (use `binaryData`). Restart the api pods after you change the ConfigMap. |
| `pvc` | `plugins.pvcName` | Mounts an existing PersistentVolumeClaim read-only at `plugins.dir`. Each api replica scans it, so use a ReadWriteMany claim when `api.replicas` is greater than 1. |

```yaml
plugins:
  source: configMap
  configMapName: aurora-plugins   # kubectl -n aurora create configmap aurora-plugins --from-file=my-app-1.0.0.aurora-plugin
```

Packages still need approval in the admin API before they take effect.

## Workloads

| Component | Values | Default | Notes |
|-----------|--------|---------|-------|
| api | `api.replicas`, `api.resources`, `api.nodeSelector`, `api.tolerations`, `api.extraEnv` | 2 | Ports 4101 (public) and 4111 (internal). `extraEnv` supports `valueFrom` (for example OIDC client secrets). |
| gateway | `gateway.replicas`, `gateway.ca.existingSecret`, `gateway.proxy.service.type`, `gateway.proxy.service.loadBalancerSourceRanges`, `gateway.proxy.service.annotations`, `gateway.proxy.tls.externalHost`, `gateway.proxy.tls.names`, `gateway.proxy.tls.existingSecret` | 2 | Ports 4103 (internal) and 4102 (proxy, Service `<fullname>-gateway-proxy`). `ClusterIP` by default. `LoadBalancer` exposes 4102 to external BYO agents and bypasses the edge; it must be layer 4 (the proxy listener is TLS-only, never terminate TLS in front of it). The proxy certificate is issued by the gateway CA for the Service names plus `tls.externalHost` and `tls.names`; `tls.existingSecret` (a `kubernetes.io/tls` Secret) replaces it with an operator certificate that must cover every name clients use. |
| hookhost | `hookhost.replicas` | 2 | Port 4104. |
| web | `web.replicas` | 2 | Port 4100. |
| runner | `runner.enabled`, `runner.replicas` (default 1), `runner.k8s.*` | enabled | Port 4105 is reachable only from the sandboxes namespace. See [runners.md](runners.md#kubernetes-backend). |

Every workload runs as non-root (UID/GID 65532), with a read-only root filesystem and all
capabilities dropped (`aurora.podSecurity`, `aurora.containerSecurity`). A
PodDisruptionBudget is rendered for each (`podDisruptionBudget.enabled`, `minAvailable: 1`).

The hook image defaults to `docker.io/alpine/k8s:1.31.2` (`hookImage`). It is a third-party
image, and FUP-M7 suggests pinning a digest or building an Aurora tools image.

## Routing

Set exactly one of these:

- **Ingress** (`ingress.enabled: true`, the default). `ingress.className`, `ingress.host`
  (default: host of `externalUrl`), `ingress.tls.secretName`, and `ingress.annotations`.
  The defaults target ingress-nginx: 64 MB body limit, buffering off for SSE and long-polls,
  600 s timeouts, and `use-forwarded-headers: "false"` so the controller overwrites
  `X-Forwarded-For`/`X-Forwarded-Proto`.
- **Gateway API** (`gatewayApi.enabled: true`, with `ingress.enabled: false`).
  `gatewayApi.parentRefs`, `gatewayApi.hostnames` and `gatewayApi.requestTimeout` (600 s).
  The Gateway implementation must enforce the body size and the header overwrite.

Routing rules are the same for both: `/v1` goes to the API (4101), `/` goes to the web
app (4100). `/internal` and `/metrics` are never routed. These are the rules
`scripts/guards/edge-routes.sh` checks.

## NetworkPolicies

With `networkPolicy.enabled: true` (default) the chart renders:

- release namespace: `<fullname>-default-deny`, `-allow-dns`, and per-component policies
  `-api`, `-web`, `-gateway`, `-hookhost`, `-runner`, `-postgres` (when the bundled database is on),
  `-connector-<id>`;
- the `migrate` hook pod (`-migrate`): egress to the database only, so `helm upgrade` can
  run migrations while the other policies are in place;
- sandboxes namespace: `default-deny` and `allow-gateway-runner`.

Set the selectors for your cluster:

- `networkPolicy.edgeNamespaceSelector` (default `kubernetes.io/metadata.name: ingress-nginx`),
  the namespace allowed to reach the API, web and connector callbacks;
- `networkPolicy.dnsNamespaceSelector` (default `kube-system`);
- `networkPolicy.kubeApiCidrs`, for runner egress to the Kubernetes API (empty allows any on 443 and 6443);
- `networkPolicy.externalEgress` (`cidr` and `except`), for OIDC, SMTP, LLM endpoints and secret stores.

Sandboxes can reach only gateway 4102 and runner 4105 (`docs/11 §kubernetes`). The kind test
checks this from inside a sandbox Pod: no DNS, no route to the API, Postgres, hookhost, gateway
4103, runner 4106 or the Kubernetes API, no service account token, a non-root user and a
read-only root filesystem.

## Connectors (optional)

`connectors` is a list. Each entry becomes a Deployment `<fullname>-connector-<id>`,
plus an Ingress if `callbackIngress.enabled`.

```yaml
connectors:
  - id: example-chat                # provisioned in the admin API first (see plugins-and-connectors.md)
    image: registry.example.org/aurora-connector-example-chat:1.0.0   # placeholder image
    callbackPort: 4190              # default 4190 (the template ignores `port`)
    tokenSecret: {name: aurora-connector-example-chat, key: AURORA_CONNECTOR_TOKEN}
    callbackIngress: {enabled: true, host: connectors.example.org, path: /example-chat}
    replicas: 1
    env: []
```

The image and the callback host above are placeholders. Use the connector image you build or
receive from its publisher.

The token Secret must exist before the install. Provisioning and approval are in
[plugins-and-connectors.md](plugins-and-connectors.md#connectors).

## Install

```bash
helm lint deploy/helm/aurora
helm template aurora deploy/helm/aurora -n aurora -f my-values.yaml > rendered.yaml   # review before applying
helm install aurora deploy/helm/aurora -n aurora --create-namespace -f my-values.yaml --wait --timeout 8m
```

`helm lint` and `helm template` are the checks to run before any values change.

Hooks run in this order: `secrets` (-10), `gateway-ca` (-9), then `migrate` (0),
which runs `aurora-api migrate` before any new API replica serves.

Minimal `my-values.yaml`:

```yaml
externalUrl: https://aurora.example.org
image:
  registry: registry.example.org/aurora
  tag: "0.1.0"
ingress:
  className: nginx
  tls: {secretName: aurora-tls}
database:
  existingSecret: aurora-database       # must contain AURORA_DATABASE_URL
  gatewayRole: aurora_gateway
runner:
  k8s:
    storageClass: fast-rwo
    runtimeClass: gvisor
```

## Test on a local kind cluster

`scripts/e2e/kind.sh` follows the steps above on a throwaway cluster. It needs `kind`, `kubectl`,
`helm`, `podman` and about 20 GB of free disk. The cluster is named `aurora-e2e`, and the kind
provider is rootless podman (`KIND_EXPERIMENTAL_PROVIDER=podman`).

```bash
bash scripts/e2e/kind.sh              # create, build, load, install, test, delete the cluster
bash scripts/e2e/kind.sh --keep       # leave the cluster for inspection
bash scripts/e2e/kind.sh --fresh      # delete an existing aurora-e2e cluster first
bash scripts/e2e/kind.sh --skip-build # reuse the localhost/aurora-e2e/* images
```

What it does:

1. Creates the cluster from `deploy/kind/kind-config.yaml` (one node, the default kindnet CNI).
2. Proves NetworkPolicy enforcement before anything else: a probe pod reaches a server pod,
   then a default-deny policy is applied and the same request must fail. The script stops if it does not.
   kindnet (`kindest/kindnetd` v20260820 in kind v0.33) enforces ingress and egress policies, including
   port-specific allows, so no extra CNI is needed. If you use an older kind node image whose
   CNI does not enforce, create the cluster with `disableDefaultCNI: true` and install a
   pinned Calico manifest before the chart (not exercised by this script).
3. Builds the service images, the web image and the sandbox image (`sandbox-min`, with the fake
   harness), and loads them with `kind load image-archive`. The sandbox image is loaded under the
   `config.runner.image` name set in the test values (see [Prerequisites](#prerequisites)).
4. Installs the chart with `deploy/kind/values-e2e.yaml`: the bundled evaluation Postgres, no
   Ingress (kind has no ingress controller), one replica per component, open sign-up, the fake
   harness allowed and a 30 s idle timeout.
5. Reaches the API with `kubectl port-forward` and runs `tests/e2e-hosted/tests/k8s.rs`:
   a chat turn answered inside a sandbox Pod, park and wake (the home PVC and a marker file
   survive), restart, and the SEC-02 and SEC-09 probes from inside a sandbox Pod, each blocked
   connection next to a positive control on the two allowed ports.

Logs of all pods, events and `describe` output are written to `target/kind-logs/` when a run fails.
Re-running is safe: an existing release and both namespaces are removed first (the evaluation
database lives on a PVC in the release namespace, so it is reset too).

## Post-install checks

```bash
kubectl -n aurora get pods
kubectl -n aurora exec deploy/aurora-api -- /usr/local/bin/aurora-api bootstrap-admin --email you@example.org
```

`bootstrap-admin` prints a one-time password-set link. It is the same subcommand as in
[install-compose.md](install-compose.md#first-administrator). The `kubectl exec` line is
not run here, and the image has no shell. The binary path is the image's `ENTRYPOINT`.

Then register a runner ([runners.md](runners.md#add-a-runner)).

## Known limits in the chart

- Changing a `plugins.configMapName` ConfigMap does not restart the API. Run
  `kubectl -n aurora rollout restart deploy/aurora-api`.
- The bundled `postgres.enabled` database is evaluation-only and leaves its resources behind
  on uninstall (see [Database](#database)).
- `helm upgrade` was not tested (separate task). The first upgrade from a release that predates the
  `-migrate` NetworkPolicy runs its migrate hook before that policy exists, so it can hang
  (seen on kind). Apply the new NetworkPolicies first (`helm template ... -s templates/networkpolicy.yaml | kubectl apply -f -`), then upgrade.
- The kind test runs a single node with `runtimeClass` unset, so gVisor and Kata are not covered.
