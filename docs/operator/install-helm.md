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

The chart has been checked with `helm lint` and `helm template` only. The `kind`
end-to-end test has not been run (`docs/PROGRESS.md`, OPS-010 and FUP-M7).

## Namespaces

- **Release namespace** (for example `aurora`): core services, hooks, ConfigMap, Secrets, NetworkPolicies.
- **Sandboxes namespace**: `sandboxes.namespace`, default `aurora-sandboxes`. Created when
  `sandboxes.create: true`. It has a ResourceQuota `sandboxes`, a LimitRange `sandboxes`,
  Pod Security Admission labels `restricted` (enforce and audit), and a runner Role
  and RoleBinding.

## Secrets

The chart uses one Secret for all generated keys. Choose one of two options.

**Option A: let the chart generate them.** Leave `secrets.existingSecret` empty.
The `<fullname>-secrets` pre-install job (`secrets`, weight -10) creates
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
- Not implemented: `AURORA_GATEWAY_DATABASE_URL`. The values comment mentions it, but no code reads it. The gateway uses `AURORA_DATABASE_URL`.

`postgres.enabled: true` renders a single-instance Postgres for evaluation. Its
pre-install hook resources are not removed by `helm uninstall` (FUP-M7, and the
template comment). Delete the StatefulSet and PVC by hand.

## Gateway CA

The `<fullname>-gateway-ca` pre-install job (`gateway-ca`, weight -9) runs
`aurora-gateway generate-ca --out /ca` and stores `ca.pem` and `ca.key` as the Secret
`<fullname>-gateway-ca`. If the Secret exists, it does nothing ("CA secret exists; not
overwriting"). To use your own CA, create a Secret with keys `ca.pem` and `ca.key` and
set `gateway.ca.existingSecret`.

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

Add `auth`, `llm`, `governance`, `secrets`, `email`, `plugins` and the other
sections the same way. The values file says so, and [llm-endpoints.md](llm-endpoints.md)
shows an example. The `plugins.dir` key is available in config, but the chart does not
mount plugin packages (**not yet implemented**, see [plugins-and-connectors.md](plugins-and-connectors.md#plugins)).

## Workloads

| Component | Values | Default | Notes |
|-----------|--------|---------|-------|
| api | `api.replicas`, `api.resources`, `api.nodeSelector`, `api.tolerations`, `api.extraEnv` | 2 | Ports 4101 (public) and 4111 (internal). `extraEnv` supports `valueFrom` (for example OIDC client secrets). |
| gateway | `gateway.replicas`, `gateway.ca.existingSecret`, `gateway.proxy.service.type`, `gateway.proxy.service.loadBalancerSourceRanges`, `gateway.proxy.service.annotations` | 2 | Ports 4103 (internal) and 4102 (proxy, Service `<fullname>-gateway-proxy`). `ClusterIP` by default. `LoadBalancer` exposes 4102 to external BYO agents and bypasses the edge. |
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
- sandboxes namespace: `default-deny` and `allow-gateway-runner`.

Set the selectors for your cluster:

- `networkPolicy.edgeNamespaceSelector` (default `kubernetes.io/metadata.name: ingress-nginx`),
  the namespace allowed to reach the API, web and connector callbacks;
- `networkPolicy.dnsNamespaceSelector` (default `kube-system`);
- `networkPolicy.kubeApiCidrs`, for runner egress to the Kubernetes API (empty allows any on 443 and 6443);
- `networkPolicy.externalEgress` (`cidr` and `except`), for OIDC, SMTP, LLM endpoints and secret stores.

Sandboxes can reach only gateway 4102 and runner 4105 (`docs/11 §kubernetes`).

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
helm install aurora deploy/helm/aurora -n aurora --create-namespace -f my-values.yaml
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

## Post-install checks

```bash
kubectl -n aurora get pods
kubectl -n aurora exec deploy/aurora-api -- /usr/local/bin/aurora-api bootstrap-admin --email you@example.org
```

`bootstrap-admin` prints a one-time password-set link. It is the same subcommand as in
[install-compose.md](install-compose.md#first-administrator). The `kubectl exec` line is
not run here, and the image has no shell. The binary path is the image's `ENTRYPOINT`.

Then register a runner ([runners.md](runners.md#add-a-runner)).

## Known issues in the chart

- `runner.k8s.nodeSelector` and `runner.k8s.tolerations` do not reach `aurora.toml`, so
  sandbox pods do not get them (verified with `helm template`). See [README.md](README.md#known-gaps-checked-against-the-code).
- `connectors[].port` in the values comment is ignored. Use `callbackPort`.
- Plugin packages cannot be mounted from the chart (**not yet implemented**).
