# Install with Helm (Kubernetes)

This is the production install described in
[docs/15 §Production](../15-deployment-operations.md#production-helm-deployhelmaurora).
The chart is `deploy/helm/aurora/` (Chart `aurora` 0.1.0, appVersion 0.1.0, `kubeVersion >=1.30.0-0`).
It renders the `aurora.toml` structure into the ConfigMap `<fullname>-config`.

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
- Images: pullable by the nodes under the names the chart uses, `<image.registry>/aurora-<name>:<image.tag>`
  (tag defaults to the chart's appVersion). **The default registry `ghcr.io/alexiri` works only if the project's CI has published
  that tag and the packages were made public**; otherwise pulls fail with `unauthorized`. Read
  [Images: publish, pull, or build and load](#images-publish-pull-or-build-and-load) before your first install.
- The sandbox image must be pullable by the nodes under the name in `config.runner.image`
  (default `aurora-sandbox:<version>`, which containerd resolves to
  `docker.io/library/aurora-sandbox:<version>`). The runner applies that setting to every
  sandbox Pod and the API reports it as the sandbox's `image`; set it to your mirrored name.

The chart is checked with `helm lint`, `helm template` and an install on a local kind
cluster (see [Test on a local kind cluster](#test-on-a-local-kind-cluster)), which runs the
hosted scenarios and the sandbox isolation probes against the `kubernetes` runner backend.

## Images: publish, pull, or build and load

The chart needs seven images: `aurora-api`, `aurora-gateway`, `aurora-hookhost`, `aurora-runner`, `aurora-connector-host` (unless `connectorHost.enabled: false`) and `aurora-web`, plus the sandbox image (named by `config.runner.image`, not by `image.registry`),
and `aurora-dev-idp` or `aurora-connector-*` only if you enable those. Pick **one** of three ways to get them.

**1. Pull the project's published images (the chart default).** The default is `image.registry: ghcr.io/alexiri` with the chart's appVersion as the tag. Those images exist **only after a CI publish** (CI publishes `master`, `sha-<short>`
and version tags once its lint, test and e2e jobs pass; see [docs/15 §Images](../15-deployment-operations.md#images)), and a package on `ghcr.io` is **private until its owner makes it public** in the package settings.
Check before you rely on it, from a machine that is not logged in to ghcr:

```bash
podman manifest inspect ghcr.io/alexiri/aurora-api:0.1.0   # an `unauthorized` error: not published, or still private
```

If the packages are private, either make them public or create a pull secret and set `image.pullSecrets` (`kubectl -n aurora create secret docker-registry regcred --docker-server=ghcr.io --docker-username=<user> --docker-password=<token with read:packages>`, then
`image: {pullSecrets: [{name: regcred}]}`). The sandbox image is pulled separately, by each sandbox Pod, with `runner.sandboxImagePullSecrets` (see [Configuration](#configuration)).

**2. Mirror to your own registry.** Copy the published images (`skopeo copy --all`) to a registry the cluster can reach and set `image.registry` and `image.tag`.

**3. Build and load them yourself.** Use this when nothing is published, or for a local cluster. The images come from the repository's Containerfiles (the commands use podman, as the project's scripts do):

```bash
REG=registry.example.org/aurora   # or localhost/aurora for kind; TAG=0.1.0
for t in api gateway hookhost runner connector-host; do
  podman build -f images/services/Containerfile --target "$t" -t "$REG/aurora-$t:$TAG" .
done
podman build -f images/services/web.Containerfile -t "$REG/aurora-web:$TAG" .
# the sandbox image: target `sandbox` (Claude Code harness), or `sandbox-min` (fake harness, for tests only)
podman build -f images/sandbox/Containerfile --target sandbox -t "$REG/aurora-sandbox:$TAG" .
```

The sandbox base defaults to the public official AlmaLinux 10 image pinned by digest. A private or mirrored base is a build-time override: add `--build-arg SANDBOX_BASE_IMAGE=<registry>/<image>:<tag>` (or set `SANDBOX_BASE_IMAGE` for `scripts/images/build-sandbox-min.sh`; `images/sandbox/Containerfile`).
The builds need several GB of free disk and a long first compile. An API image built this way carries **no signed built-in packages**: without the project signing key the API refuses them
([apps-and-catalogue.md](apps-and-catalogue.md#the-built-in-catalogue)); upload the packages you need as ordinary plugins.

*Real cluster with a private registry.* Push the images and point the chart at them:

```bash
for i in api gateway hookhost runner web sandbox; do podman push "$REG/aurora-$i:$TAG"; done
```

```yaml
image:
  registry: registry.example.org/aurora
  tag: "0.1.0"
  pullSecrets: [{name: regcred}]           # a docker-registry Secret in the release namespace
config:
  runner:
    image: registry.example.org/aurora/aurora-sandbox:0.1.0
runner:
  sandboxImagePullSecrets: [sandbox-registry]   # a docker-registry Secret in the sandboxes namespace
```

*kind (no registry).* Load the images into the cluster's nodes and tell the chart not to pull. `scripts/e2e/kind.sh` and `deploy/dev/kind-dev.sh` do exactly this; their load step is:

```bash
export KIND_EXPERIMENTAL_PROVIDER=podman        # kind with the rootless podman provider
for i in aurora-api aurora-gateway aurora-hookhost aurora-runner aurora-connector-host aurora-web aurora-sandbox; do
  podman save -o "/tmp/$i.tar" "$REG/$i:$TAG" && kind load image-archive "/tmp/$i.tar" --name <cluster> && rm -f "/tmp/$i.tar"
done
```

```yaml
image: {registry: localhost/aurora, tag: local, pullPolicy: Never}
config: {runner: {image: localhost/aurora/aurora-sandbox:local}}
```

`image.registry: ""` means bare image names (`aurora-api:<tag>`), which is only right when the nodes already hold images under exactly those names.
For a ready-made path use the scripts instead of typing this: `bash scripts/e2e/kind.sh --release-images` builds the release images, loads them and runs the tests ([below](#test-on-a-local-kind-cluster)),
and [install-kind-dev.md](install-kind-dev.md) keeps a persistent cluster with fast rebuilds. The commands above are modelled on those scripts and were not run as part of writing this page.

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
`AURORA_CONNECTOR_TOKEN`, `AURORA_CONNECTOR_HOST_TOKEN`. With `postgres.enabled: true` it also writes
`POSTGRES_PASSWORD` and `AURORA_DATABASE_URL`. The job runs on `helm install` and on every
`helm upgrade` (it only adds keys that are missing, so an upgrade from a release without a
connector host creates `AURORA_CONNECTOR_HOST_TOKEN`).

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
| `AURORA_CONNECTOR_HOST_TOKEN` (`auh_…`) | connector host, and the API bootstrap (`api.bootstrap.connectorHostToken`, default true); only with `connectorHost.enabled` |

`AURORA_CONNECTOR_TOKEN` is generated into the shared Secret, but no chart workload reads it.
Each self-hosted connector takes its token from its own `tokenSecret` (see [Connectors](#connectors-optional)).
The managed connectors run by the connector host have no token of their own: the host's single
`auh_` token covers them (see [Connector host](#connector-host)).

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
- `database.cidrs` and `database.port` (default 5432) set the egress rule to the database in the NetworkPolicies.
- No code reads `AURORA_GATEWAY_DATABASE_URL`. The gateway reads `AURORA_DATABASE_URL`: from the main
  Secret by default, or from `database.gatewayExistingSecret` when you set the least-privilege pair below.

### Least-privilege gateway role (recommended)

The gateway is the most exposed service, so it should not log in as the API's full-privilege user.
`database.gatewayRole` and `database.gatewayExistingSecret` go together: set both or neither
(`helm install` fails with a message if only one is set). **Without the pair, the gateway uses the
main database user (not least privilege).** `aurora-api migrate` only *grants* the role; it never
creates it (creating a login role needs `CREATEROLE` and a password, which the API user should not
hold). The DBA does this, in order, before `helm install`:

1. Create the role with a password (as a Postgres superuser or a `CREATEROLE` user), in the same cluster
   as the Aurora database:

   ```sql
   CREATE ROLE aurora_gateway LOGIN PASSWORD '<strong password>';
   ```

   Give it no other attributes. Do not grant it superuser, `CREATEROLE`, `CREATEDB` or any privilege on
   the Aurora tables: `migrate` grants exactly the privileges in `docs/03 §Gateway database role grants`.
2. Create the gateway Secret (same namespace as the release), with the same host and database as the
   main URL but the gateway role's login:

   ```bash
   kubectl -n aurora create secret generic aurora-gateway-database \
     --from-literal=AURORA_DATABASE_URL='postgres://aurora_gateway:<password>@db.example.org:5432/aurora'
   ```
3. Set both values (see the example values below): `database.gatewayRole: aurora_gateway` and
   `database.gatewayExistingSecret: aurora-gateway-database`.
4. `helm install`. The `migrate` hook runs `aurora-api migrate`, which applies the grants. If the role
   is missing it stops with `gateway role "aurora_gateway" does not exist: create it (CREATE ROLE
   aurora_gateway LOGIN PASSWORD '…') or unset database.gateway_role`.

The evaluation Postgres (`postgres.enabled`) and the dev profile set neither value, so the gateway
uses the main user there: it is the simplest correct option for a throwaway database, where nobody
creates roles. To exercise the pair on the evaluation database anyway, mount an init script with
`postgres.initScriptsConfigMap` (a ConfigMap with a `.sql` file containing the `CREATE ROLE`, run once on
an empty data directory); `scripts/e2e/kind.sh` does this.

`postgres.enabled: true` renders a single-instance Postgres for evaluation. Its StatefulSet,
Service and PVC are pre-install hook resources, so `helm uninstall` leaves them and the data
stays on the PVC (FUP-M7). This is deliberate (D-100): the database must exist before the `migrate`
pre-install hook runs, which only a hook can guarantee, and Helm does not manage or delete hook
resources. Rendering them as normal chart resources would not be safe for upgrades: a release that
already has them as hooks would fail `helm upgrade` with "exists and cannot be imported into the current
release", and the chart cannot adopt them. Evaluation installs therefore keep the hook form, and the
chart documents the leftovers (`NOTES.txt` prints the cleanup commands).

What stays behind after `helm uninstall`: the StatefulSet `<fullname>-postgres`, the Service
`<fullname>-postgres` and the PVC `data-<fullname>-postgres-0` (all your evaluation data). `helm upgrade`
never touches them (they exist only at install time), so changing `postgres.image` or `postgres.storage`
on an existing release has no effect. Delete and reinstall, or edit the StatefulSet yourself. Remove
everything by hand:

```bash
# Release "aurora" (fullname "aurora"); otherwise the names start with <fullname>-postgres
kubectl -n aurora delete statefulset,service aurora-postgres
kubectl -n aurora delete pvc data-aurora-postgres-0
```

Use an external PostgreSQL for anything that keeps its data.

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

### Sandbox size

Each hosted sandbox gets `config.runner.sandbox_cpus` (default 1) CPUs, `sandbox_memory_mib` (2048), `sandbox_pids` (512) and `home_gib` (10) unless an agent overrides them; requests equal limits, and the sandbox namespace quota still applies. Size them to your nodes: a sandbox that does not fit stays `Unschedulable`, and the runner logs the scheduler's reason when its capacity wait expires.

### Gateway proxy addresses

Two addresses reach the gateway proxy listener (port 4102), for two audiences:

| Key | Audience | Default | Set it when |
| --- | --- | --- | --- |
| `gateway.advertised_proxy_url` | BYO agents, outside the cluster (`container_config.proxy_url`) | `https://<host of externalUrl>:4102` | the proxy is published under another name than the web host (a layer-4 load balancer or NodePort hostname). Add that name to `gateway.proxy.tls.externalHost` or `names` |
| `gateway.sandbox_proxy_url` | hosted sandboxes, inside the cluster (the boot document) | `https://aurora-gateway:4102` (the chart runs the Kubernetes backend) | almost never: sandboxes have no DNS, the runner aliases `aurora-gateway` to the proxy Service, and the chart puts that name in the proxy certificate. Override only for a different in-cluster name, and make sure the name is on the certificate (`gateway.proxy.tls.names`; the gateway also adds its host itself) |

Do not point `advertised_proxy_url` at an in-cluster name to make sandboxes work: BYO agents could not
resolve it. Sandboxes always verify the proxy certificate against the host of the sandbox address.

Add `auth`, `llm`, `governance`, `secrets`, `email` and the other sections the same way.
The values file says so, and [llm-endpoints.md](llm-endpoints.md) shows an example.
`plugins.dir` is set from the top-level `plugins.dir` value (default `/var/lib/aurora/plugins`),
so do not set it in `config`. `runner.k8s.nodeSelector` and `runner.k8s.tolerations` are
rendered into `[runner.k8s]` as `node_selector` and `tolerations`.

For a sandbox image in a private registry, set `runner.sandboxImagePullSecrets` (a list of
Secret names), rendered as `[runner.k8s] image_pull_secrets`. The chart does not create the
Secrets and does not reuse the service `imagePullSecrets`: create them in the sandboxes
namespace (`sandboxes.namespace`), for example
`kubectl -n aurora-sandboxes create secret docker-registry sandbox-registry --docker-server=registry.example.org --docker-username=<user> --docker-password=<token>`
(see [runners.md](runners.md#kubernetes-backend)).

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
| api | `api.replicas`, `api.resources`, `api.nodeSelector`, `api.tolerations`, `api.extraEnv` | 2 | Ports 4101 (public) and 4111 (internal). `extraEnv` supports `valueFrom` (for example OIDC client secrets). The pre-install/pre-upgrade migrate Job gets the same `api.extraEnv`, because `aurora-api migrate` runs the same configuration check (a missing `*_env` secret would fail it). The referenced Secrets must exist before `helm install`. |
| gateway | `gateway.replicas`, `gateway.ca.existingSecret`, `gateway.proxy.service.type`, `gateway.proxy.service.loadBalancerSourceRanges`, `gateway.proxy.service.annotations`, `gateway.proxy.tls.externalHost`, `gateway.proxy.tls.names`, `gateway.proxy.tls.existingSecret` | 2 | Ports 4103 (internal) and 4102 (proxy, Service `<fullname>-gateway-proxy`). `ClusterIP` by default. `LoadBalancer` exposes 4102 to external BYO agents and bypasses the edge; it must be layer 4 (the proxy listener is TLS-only, never terminate TLS in front of it). The proxy certificate is issued by the gateway CA for the Service names plus `tls.externalHost` and `tls.names`; `tls.existingSecret` (a `kubernetes.io/tls` Secret) replaces it with an operator certificate that must cover every name clients use. |
| hookhost | `hookhost.replicas` | 2 | Port 4104. |
| web | `web.replicas` | 2 | Port 4100. |
| runner | `runner.enabled`, `runner.replicas` (default 1), `runner.k8s.*` | enabled | Port 4105 is reachable only from the sandboxes namespace. See [runners.md](runners.md#kubernetes-backend). |
| connector host | `connectorHost.enabled`, `connectorHost.replicas`, `connectorHost.resources`, `connectorHost.testTypes`, `connectorHost.nodeSelector`, `connectorHost.tolerations`, `connectorHost.extraEnv` | enabled, 1 | Ports 4110 (platform callbacks, Service `<fullname>-connector-host`, routed by the edge) and 4192 (health and metrics, no Service). See [Connector host](#connector-host). |

### Connector host

`aurora-connector-host` runs the connector types built into Aurora (phase 4: Mattermost) for the
managed connectors an instance administrator adds in the dashboard
([plugins-and-connectors.md](plugins-and-connectors.md#add-a-managed-connector-dashboard)). It
is on by default; set `connectorHost.enabled: false` to run no managed connectors (self-hosted
connectors, [below](#connectors-optional), do not need it).

- **Token.** The secrets hook generates `AURORA_CONNECTOR_HOST_TOKEN` (`auh_…`) into the instance
  Secret, once (existing values are never overwritten), and the API receives it as
  `AURORA_BOOTSTRAP_CONNECTOR_HOST_TOKEN` (`api.bootstrap.connectorHostToken`, default `true`).
  The hook now also runs on `helm upgrade`, so a release installed before the connector host gets
  the new key. With `secrets.existingSecret` you add `AURORA_CONNECTOR_HOST_TOKEN` yourself before
  enabling the host: `auh_` followed by random characters (the chart generates `auh_` plus 48 hex digits).
- **Config.** The chart renders `[connector_host]` into `aurora.toml`: `api_url` is the in-cluster
  API Service, `test_types` comes from `connectorHost.testTypes`. `connector_host.listen`
  (`0.0.0.0:4110`) and `connector_host.ops_port` (`4192`) keep their defaults.
  Every replica runs every enabled managed connector.
- **`connectorHost.testTypes`** (default `false`) also announces the `fake` test type and mounts a
  test control API on the ops port. The host warns at start. Development and end-to-end tests only;
  `deploy/kind/values-e2e.yaml` sets it.
- **Edge.** `/connectors` goes to the host on 4110 (Ingress and HTTPRoute), next to `/v1` and
  `/`. The URL a platform such as Mattermost calls back is
  `<externalUrl>/connectors/<id>/…`; see [Routing](#routing).
- **NetworkPolicy** `<fullname>-connector-host`: ingress on 4110 from the edge namespace (and, with
  `networkPolicy.cilium.enabled`, from `networkPolicy.cilium.edgeEntities`); egress to the API on
  4101, DNS, and external addresses (the chat platform). A platform on a private address range needs
  `networkPolicy.connectorHostExtraEgress`, a list of raw NetworkPolicy `egress` entries, for example:

  ```yaml
  networkPolicy:
    connectorHostExtraEgress:
      - to: [{ipBlock: {cidr: 10.20.0.0/24}}]
        ports: [{protocol: TCP, port: 443}]
  ```

  Every outbound connection of the host goes through this policy, so a platform you do not open
  here stays unreachable.
- **Image.** `aurora-connector-host` is a separate image like the others
  ([Images](#images-publish-pull-or-build-and-load)). The release build embeds the manifest
  signatures made with the project key; a build without the key announces unsigned manifests,
  which the API refuses, so the dashboard lists no types.

Every workload runs as non-root (UID/GID 65532), with a read-only root filesystem and all
capabilities dropped (`aurora.podSecurity`, `aurora.containerSecurity`). A
PodDisruptionBudget is rendered for each (`podDisruptionBudget.enabled`, `minAvailable: 1`).

The hook image (`hookImage`, used by the secrets and gateway-ca hook Jobs and the API `seed-plugins`
init container) is `docker.io/alpine/k8s:1.31.2` **pinned by the multi-arch index digest**
(`…:1.31.2@sha256:d489e3c7…`, amd64 and arm64), so a retagged or compromised upstream tag cannot
change what runs in your cluster. It is still a third-party image: air-gapped or policy-restricted
clusters should mirror it to their own registry by digest (`skopeo copy --all`) and set `hookImage` to the
mirror, keeping the `@sha256:` suffix. It must provide `sh`, `cp`, `kubectl`, `base64`, `head`, `od` and `tr`.
To bump it, resolve the new tag's index digest with
`skopeo inspect --raw docker://docker.io/alpine/k8s:<tag> | sha256sum` (or `podman manifest inspect`) and
update `values.yaml`. D-100 records why a pin was chosen over an Aurora-built tools image.

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

Routing rules are the same for both: `/v1` goes to the API (4101), `/connectors` goes to the
connector host (4110, the platform callbacks of managed connectors; only when
`connectorHost.enabled`), `/` goes to the web app (4100). `/internal` and `/metrics` are never
routed. These are the rules
`scripts/guards/edge-routes.sh` checks.

### Behind an external reverse proxy

When users reach Aurora through a reverse proxy outside the cluster (for example nginx on its own
VM), the proxy needs one in-cluster entry point that it can reach. The api and web Services are
`ClusterIP` only, so expose them through a Gateway (or an ingress controller) and point the proxy at
that address, not at the pods:

1. **Gateway** (owned by the cluster, not by this chart). With Cilium, use the `cilium`
   GatewayClass. On an OpenStack cloud each Gateway gets a load balancer (it needs load-balancer
   quota); without quota, Cilium can run the Gateway on the node host network instead. Give it an
   HTTPS listener with a certificate for the public name, so that session cookies never cross the
   network between the proxy and the cluster in clear text:

   ```yaml
   apiVersion: gateway.networking.k8s.io/v1
   kind: Gateway
   metadata: {name: aurora, namespace: aurora}
   spec:
     gatewayClassName: cilium
     listeners:
       - name: https
         protocol: HTTPS
         port: 443
         hostname: aurora.example.org          # the public name users type
         tls: {mode: Terminate, certificateRefs: [{name: aurora-edge-tls}]}
         allowedRoutes: {namespaces: {from: Same}}
   ```

   `kubectl -n aurora get gateway aurora` shows the address once it is programmed.
2. **Chart values**: `ingress.enabled: false`, `gatewayApi.enabled: true`,
   `gatewayApi.parentRefs: [{name: aurora}]`, `externalUrl: https://aurora.example.org`. On Cilium
   also set `networkPolicy.cilium.enabled: true`. Cilium's Gateway Envoy connects to the backends
   as the `ingress` entity, which the `edgeNamespaceSelector` rule never matches. The option adds
   one `CiliumNetworkPolicy` per edge port that allows `networkPolicy.cilium.edgeEntities`
   (default `[ingress]`; use `[host, remote-node]` for a host-network ingress controller such as
   ingress-nginx run as a `hostNetwork` DaemonSet).
3. **Proxy**: forward everything to the Gateway address over HTTPS, keep the `Host` header, and do
   not buffer (SSE and long-polls):

   ```nginx
   upstream aurora_edge { server <gateway address>:443; keepalive 16; }
   server {
     listen 443 ssl http2;
     server_name aurora.example.org;
     # ssl_certificate / ssl_certificate_key for aurora.example.org
     client_max_body_size 64m;
     location / {
       proxy_pass https://aurora_edge;
       proxy_ssl_server_name on;
       proxy_ssl_name aurora.example.org;
       proxy_ssl_verify on;                       # trust the Gateway certificate's CA
       proxy_ssl_trusted_certificate /etc/nginx/aurora-edge-ca.pem;
       proxy_http_version 1.1;
       proxy_set_header Connection "";
       proxy_set_header Host $host;
       proxy_set_header X-Forwarded-Proto https;
       proxy_set_header X-Forwarded-For $remote_addr;   # overwrite, never append the client's
       proxy_buffering off;
       proxy_request_buffering off;
       proxy_read_timeout 600s;
       proxy_send_timeout 600s;
     }
   }
   ```
4. **Client addresses** (`config.server.trusted_proxies`). The API takes the rightmost
   `X-Forwarded-For` hop that is not in this list. Each hop that the Envoy sees as its peer is
   appended, so list the proxy's address, plus the Gateway data path (the private pod ranges are
   already in the default). A layer-4 load balancer that rewrites the source address (an OpenStack
   load balancer without the PROXY protocol does) puts its own address in the chain. Either enable
   the PROXY protocol between the load balancer and the Gateway, so the Envoy sees the proxy's
   address, or list the load balancer's address and allow only the proxy to reach the load
   balancer. Otherwise anyone who can reach it directly can choose their own client address.
   Check the result with `GET /v1/me`: `client_address` must be your own address, not the
   proxy's or the load balancer's.

   The `location /` above already forwards `/connectors/` with everything else. A proxy that
   forwards only selected paths must forward **`/connectors/`** as well (next to `/v1/` and `/`),
   with the same settings: the platforms of managed connectors, such as Mattermost, call back at
   `https://aurora.example.org/connectors/<id>/…`, and the Gateway routes that prefix to the
   connector host. Without it, buttons, menus and slash commands of managed connectors do not work.

BYO agents connect to the gateway proxy on 4102, which is TLS end to end and never goes through
the edge. Publish it with `gateway.proxy.service.type: LoadBalancer`, or pass it through the
external proxy at layer 4 (an nginx `stream` block with no TLS termination). See
[Gateway proxy addresses](#gateway-proxy-addresses).

## NetworkPolicies

With `networkPolicy.enabled: true` (default) the chart renders:

- release namespace: `<fullname>-default-deny`, `-allow-dns`, and per-component policies
  `-api`, `-web`, `-gateway`, `-hookhost`, `-runner`, `-connector-host`, `-postgres` (when the
  bundled database is on), `-connector-<id>` (self-hosted connectors);
- the `migrate` hook pod (`-migrate`): egress to the database only, so `helm upgrade` can
  run migrations while the other policies are in place;
- sandboxes namespace: `default-deny` and `allow-gateway-runner`.

Set the selectors for your cluster:

- `networkPolicy.edgeNamespaceSelector` (default `kubernetes.io/metadata.name: ingress-nginx`),
  the namespace allowed to reach the API, web and connector callbacks;
- `networkPolicy.dnsNamespaceSelector` (default `kube-system`);
- `networkPolicy.kubeApiCidrs`, for runner egress to the Kubernetes API (empty allows any), and
  `networkPolicy.kubeApiPorts` (default `[443, 6443]`), the API server's endpoint port. Check it with
  `kubectl get endpointslice -n default -l kubernetes.io/service-name=kubernetes`;
- `networkPolicy.externalEgress` (`cidr` and `except`), for OIDC, SMTP, LLM endpoints and secret stores.

**Cilium:** set `networkPolicy.cilium.enabled: true`. Cilium never matches the API server or
cluster nodes with a NetworkPolicy `ipBlock` (they are the `kube-apiserver` / `remote-node`
entities), so without it the runner times out reaching the Kubernetes API at start:
`kubernetes API: cannot list pods in namespace … ServiceError: client error (Connect)`. The option
adds one `CiliumNetworkPolicy` that allows only runner → `toEntities: [kube-apiserver]` on
`kubeApiPorts`. Cilium unions it with the NetworkPolicies, so nothing else opens. It needs the
`cilium.io/v2` CRDs. Anything else that must reach a node address (a database on a node IP, a
NodePort) has the same Cilium behaviour. With the option on, the edge ports also accept
`networkPolicy.cilium.edgeEntities` (default `[ingress]`, Cilium's own Gateway API / Ingress Envoy),
one `CiliumNetworkPolicy` per edge-facing component (api 4101, web 4100, connector host 4110,
dev-idp 4107, self-hosted connector callbacks); see [Behind an external reverse proxy](#behind-an-external-reverse-proxy).

Sandboxes can reach only gateway 4102 and runner 4105 (`docs/11 §kubernetes`). The kind test
checks this from inside a sandbox Pod: no DNS, no route to the API, Postgres, hookhost, gateway
4103, runner 4106 or the Kubernetes API, no service account token, a non-root user and a
read-only root filesystem.

## Connectors (optional)

This section is for **self-hosted** connectors: services you deploy yourself, with their own
token. For the connectors Aurora ships (Mattermost), do not list anything here: add them in the
dashboard and the [connector host](#connector-host) runs them.

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
  registry: registry.example.org/aurora   # the default ghcr.io/alexiri works only after a CI publish and public packages
  tag: "0.1.0"
ingress:
  className: nginx
  tls: {secretName: aurora-tls}
database:
  existingSecret: aurora-database       # must contain AURORA_DATABASE_URL
  # The least-privilege gateway login: both or neither (see Database). The DBA creates the role and
  # the Secret (key AURORA_DATABASE_URL, logging in as that role) first.
  gatewayRole: aurora_gateway
  gatewayExistingSecret: aurora-gateway-database
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

A persistent dev cluster with the single URL, a mock model and fast image iteration is
[install-kind-dev.md](install-kind-dev.md) (phase 1, OPS-017).

## Real-cluster install checklist (AUDIT-002 / owner)

Everything above is exercised on a local kind cluster only. P2-OPS-1 could not run it on a real cluster:
nobody has provided one, and credentials were not available. This is the checklist to run on a real
cluster, with a real ingress controller, a CNI that enforces NetworkPolicy, a default StorageClass and an
external PostgreSQL 16. Record the date, the Kubernetes version, the CNI and the result of each line.

1. **Preconditions.** `kubectl version`, `helm version` (3.x), `kubectl auth can-i create clusterrole`
   (the chart needs namespace admin; see [Namespaces](#namespaces)). CNI enforces NetworkPolicy
   (run steps 1 to 2 of the [kind script](#test-on-a-local-kind-cluster) logic by hand: a probe pod
   reaches a server pod, a default-deny policy then blocks it). Images are pushed to a registry the nodes
   can pull from, by digest or an immutable tag, and `image.registry`/`image.tag` point at them
   (see [Images](#images-publish-pull-or-build-and-load): the `ghcr.io/alexiri` default needs a CI publish and public packages).
   The hook image resolves from the nodes (or is mirrored, see [Workloads](#workloads)).
2. **Inputs.** Create the namespaces, the Secrets ([Secrets](#secrets)), the database Secret, and a
   `values.yaml` with `externalUrl`, `ingress.host`/`ingress.tls.secretName`, an OIDC provider under
   `config.auth.oidc_providers`, `postgres.enabled: false`, `devIdp.enabled: false`.
3. **Static checks.** `helm lint deploy/helm/aurora -f values.yaml` and
   `helm template aurora deploy/helm/aurora -f values.yaml | kubeconform -strict -ignore-missing-schemas`.
4. **Install.** `helm install aurora deploy/helm/aurora -n aurora -f values.yaml --wait --timeout 10m`.
   Expect the hook Jobs (`secrets`, `gateway-ca`, `migrate`) to complete and every Deployment to be Ready
   (`kubectl -n aurora get pods,jobs`). The hook image pulls by digest.
5. **Edge.** `curl -fsS https://<host>/v1/instance` returns 200 (the instance info with the sign-in providers) over a valid certificate. `/healthz` is served by each service but isn't routed through the edge. The web UI loads
   `/` and the OIDC sign-in redirects to the provider and back. The gateway proxy Service type matches
   what you set (`ClusterIP`, or `LoadBalancer` with `loadBalancerSourceRanges`).
6. **Bootstrap and first use.** `kubectl -n aurora exec deploy/aurora-api -- /usr/local/bin/aurora-api bootstrap-admin --email <you>`
   (see [Post-install checks](#post-install-checks)), sign in, register a runner ([runners.md](runners.md)),
   start a sandbox, send one chat turn, park it and wake it.
7. **Isolation.** From inside a sandbox Pod, confirm the SEC-02 and SEC-09 probes from the kind e2e
   (`tests/e2e-hosted/tests/k8s.rs`): no DNS, no route to the API, the database, hookhost or the cloud
   metadata address; only the two allowed ports answer. If `runtimeClass` is set (gVisor, Kata),
   run the same probes with it.
8. **Upgrade.** Change one value (for example `web.replicas`) and `helm upgrade aurora deploy/helm/aurora -n aurora -f values.yaml --wait`.
   Before the first upgrade from a release that predates the `-migrate` NetworkPolicy, apply the new policies first
   (see [Known limits](#known-limits-in-the-chart)). Verify the pods roll and the chat from step 6 still works.
9. **Uninstall.** `helm uninstall aurora -n aurora`; check what remains (the namespaces, the Secrets you
   created, the PVCs, and with `postgres.enabled` the evaluation database, see [Database](#database)).
10. **Backup and restore.** One restore drill from [backups.md](backups.md).

Open limitation (P2-OPS-1): none of the lines above has been run on a real cluster yet.

## Post-install checks

```bash
kubectl -n aurora get pods
kubectl -n aurora exec deploy/aurora-api -- /usr/local/bin/aurora-api bootstrap-admin --email you@example.org
```

All sign-in is OIDC (phase 1, RFE-004): configure at least one provider under `config.auth.oidc_providers`
(`docs/04 §OIDC`), or the API refuses to start (`no_oidc_provider`). `bootstrap-admin` **pre-provisions** an
instance administrator with a verified e-mail and no credential, and prints the sign-in URL
(`<externalUrl>/auth/login`); the person's first OIDC sign-in with that e-mail binds their identity. It refuses to
run when no provider is configured. The `kubectl exec` line is not run here, and the image has no shell. The binary
path is the image's `ENTRYPOINT`. In the **dev** images of `deploy/dev/kind-dev.sh` the binary is
`/usr/local/bin/aurora-service` instead (`deploy/dev/thin.Containerfile`; `kind-dev.sh bootstrap-admin <email>` wraps it).

Then register a runner ([runners.md](runners.md#add-a-runner)).

## Known limits in the chart

- Changing a `plugins.configMapName` ConfigMap does not restart the API. Run
  `kubectl -n aurora rollout restart deploy/aurora-api`.
- The bundled `postgres.enabled` database is evaluation-only and leaves its resources behind
  on uninstall (see [Database](#database)).
- `helm upgrade` from the previous chart to this one was run on a kind cluster (P2-OPS-1): it completes,
  the pods and the evaluation database stay up. The first upgrade from a release that predates the
  `-migrate` NetworkPolicy runs its migrate hook before that policy exists, so it can hang
  (seen on kind). Apply the new NetworkPolicies first (`helm template ... -s templates/networkpolicy.yaml | kubectl apply -f -`), then upgrade.
- The kind test runs a single node with `runtimeClass` unset, so gVisor and Kata are not covered.
