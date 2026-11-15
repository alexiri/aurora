#!/usr/bin/env bash
# OPS-011: Kubernetes e2e on a local kind cluster (rootless podman provider).
#
#   scripts/e2e/kind.sh [--keep] [--fresh] [--skip-build] [--release-images] [--no-upgrade-legs] [-- <cargo test filter/args>]
#
#   --keep        leave the cluster (and the Helm release) in place when done
#   --fresh       delete the cluster aurora-e2e first (default: reuse it, reinstall the release)
#   --skip-build  reuse the localhost/aurora-e2e/* images already in podman storage
#   --release-images  build the Rust service images from images/services/Containerfile (release profile, what CI
#                 publishes). Default (OPS-023): the OPS-017 dev path, i.e. the builder container with a
#                 persistent target volume + thin distroless images (deploy/dev/kind-dev.sh), debug profile.
#                 CI (and anything that must prove the shipped image) passes --release-images or sets
#                 AURORA_E2E_RELEASE_IMAGES=1.
#
# Steps: kind cluster (deploy/kind/kind-config.yaml) -> NetworkPolicy enforcement check with a
# probe pod -> build + `kind load image-archive` the service and sandbox images -> `helm install`
# with deploy/kind/values-e2e.yaml (docs/operator/install-helm.md) -> readiness -> port-forward the
# API -> tests/e2e-hosted/tests/k8s.rs (chat turn in a sandbox Pod, park/wake, restart, SEC-02 and
# SEC-09 probes from inside the sandbox Pod; a managed `fake` connector added through the admin API and
# run by the chart's connector host, one message round trip, disable stops it). Logs are collected under target/kind-logs/ on failure.
#
# Everything created is named aurora-e2e*: cluster `aurora-e2e`, images `localhost/aurora-e2e/*`,
# namespaces `aurora` and `aurora-sandboxes`, probe namespace `aurora-e2e-npcheck`.
#
# OPS-024: after the test phase, two `helm upgrade` legs (value-only: `web.replicas` 1 -> 2 -> 1)
# prove the hook path on an existing release: on upgrade the pre-install,pre-upgrade hooks (the
# `secrets` job, its RBAC and its hook-rendered NetworkPolicy/CiliumNetworkPolicy) run under the
# previous release's default-deny policies, exactly the production condition of the 2026-11-11
# incident. `AURORA_E2E_UPGRADE_LEGS=0` skips the legs (fresh-install flow only).
#
# Environment overrides (unset = the values above, so the script behaves as before). Use them to run
# next to another lane's cluster without touching its cluster, images or ports:
#   AURORA_E2E_CLUSTER     kind cluster name (default aurora-e2e); --fresh deletes only this one
#   AURORA_E2E_CONTEXT     kubectl context (default kind-$AURORA_E2E_CLUSTER)
#   AURORA_E2E_REGISTRY    image prefix (default localhost/aurora-e2e)
#   AURORA_KIND_PORT       host port forwarded to the API (default 14101)
#   AURORA_E2E_IDP_PORT    host port forwarded to the dev IdP, also its issuer URL (default 4107)
#   AURORA_KIND_HOST_OPS_PORT  host port forwarded to the connector host's ops port (default 14192)
#   AURORA_E2E_PREVIEWS_DOMAIN  (P4-PRV-2) unset (default): stock run, no relay. Set to a DNS
#                          suffix (e.g. previews.e2e.aurora.dev): installs with relay.enabled=true
#                          + config.previews.enabled (that domain), builds/loads the relay image
#                          and port-forwards the relay edge; the k6 preview round-trip scenario
#                          (tests/e2e-hosted/tests/k8s.rs) runs instead of skipping. The suffix
#                          is synthetic (nothing resolves it): the tests speak directly to the
#                          port-forwarded relay with an explicit Host header.
set -uo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

CLUSTER="${AURORA_E2E_CLUSTER:-aurora-e2e}"
export KIND_EXPERIMENTAL_PROVIDER=podman
CTX="${AURORA_E2E_CONTEXT:-kind-$CLUSTER}"
NS=aurora
SBX_NS=aurora-sandboxes
RELEASE=aurora
FULL=aurora            # chart fullname: the release name contains "aurora"
REGISTRY="${AURORA_E2E_REGISTRY:-localhost/aurora-e2e}"
TAG=e2e
LOCAL_PORT="${AURORA_KIND_PORT:-14101}"
IDP_PORT="${AURORA_E2E_IDP_PORT:-4107}"
export AURORA_KIND_HOST_OPS_PORT="${AURORA_KIND_HOST_OPS_PORT:-14192}"
# The registry, the sandbox image and the dev IdP issuer in values-e2e.yaml are overridden with the
# values above (identical to the file's own when the AURORA_E2E_* variables are unset).
HELM_OVERRIDES=(--set "image.registry=$REGISTRY" --set "config.runner.image=$REGISTRY/aurora-sandbox:$TAG" \
  --set "devIdp.issuer=http://localhost:$IDP_PORT")
# (P4-PRV-2) previews on: the chart's fail-guard needs relay.enabled and config.previews.enabled
# to agree; relay.domain is the preview DNS suffix (config.previews.domain).
PREVIEWS_DOMAIN="${AURORA_E2E_PREVIEWS_DOMAIN:-}"
if [ -n "$PREVIEWS_DOMAIN" ]; then
  HELM_OVERRIDES+=(--set relay.enabled=true --set "relay.domain=$PREVIEWS_DOMAIN" --set config.previews.enabled=true)
fi
LOGDIR="$ROOT/target/kind-logs"
WORK="$ROOT/target/kind"
KEEP=0; FRESH=0; SKIP_BUILD="${SKIP_IMAGE_BUILD:-0}"; RELEASE_IMAGES="${AURORA_E2E_RELEASE_IMAGES:-0}"
UPGRADE_LEGS="${AURORA_E2E_UPGRADE_LEGS:-1}"
CARGO_ARGS=()
while [ $# -gt 0 ]; do
  case "$1" in
    --keep) KEEP=1 ;;
    --fresh) FRESH=1 ;;
    --skip-build) SKIP_BUILD=1 ;;
    --release-images) RELEASE_IMAGES=1 ;;
    --no-upgrade-legs) UPGRADE_LEGS=0 ;;
    --) shift; CARGO_ARGS=("$@"); break ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
  shift
done

k() { kubectl --context "$CTX" "$@"; }
log() { echo "==> $*" >&2; }
PF_PID=""
STATUS=1

collect_logs() {
  rm -rf "$LOGDIR"; mkdir -p "$LOGDIR"
  for ns in "$NS" "$SBX_NS"; do
    k -n "$ns" get all,pvc,networkpolicy,events -o wide >"$LOGDIR/$ns-get.txt" 2>&1
    k -n "$ns" describe pods >"$LOGDIR/$ns-describe.txt" 2>&1
    for p in $(k -n "$ns" get pods -o name 2>/dev/null); do
      k -n "$ns" logs "$p" --all-containers --tail=500 >"$LOGDIR/$ns-${p#pod/}.log" 2>&1
    done
  done
  k get events -A --sort-by=.lastTimestamp >"$LOGDIR/events.txt" 2>&1
  log "logs collected in $LOGDIR"
}

cleanup() {
  [ -n "$PF_PID" ] && kill "$PF_PID" >/dev/null 2>&1
  [ -n "${IDP_PF_PID:-}" ] && kill "$IDP_PF_PID" >/dev/null 2>&1
  [ -n "${RELAY_PF_PID:-}" ] && kill "$RELAY_PF_PID" >/dev/null 2>&1
  if [ "$STATUS" -ne 0 ] && kind get clusters 2>/dev/null | grep -qx "$CLUSTER"; then collect_logs; fi
  if [ "$KEEP" -eq 0 ]; then
    log "tearing down cluster $CLUSTER (use --keep to skip)"
    kind delete cluster --name "$CLUSTER" >&2 2>&1
  else
    log "--keep: cluster $CLUSTER left running (kubectl --context $CTX)"
  fi
  rm -rf "$WORK/archives"
}
trap cleanup EXIT

for t in kind kubectl helm podman cargo; do command -v "$t" >/dev/null || { echo "missing tool: $t" >&2; exit 1; }; done
free_kb=$(df --output=avail -k . | tail -1)
if [ "$free_kb" -lt $((20 * 1024 * 1024)) ]; then echo "less than 20 GB free; stopping" >&2; exit 1; fi
mkdir -p "$WORK"

# ---- 1. cluster -------------------------------------------------------------------------
if [ "$FRESH" -eq 1 ] && kind get clusters 2>/dev/null | grep -qx "$CLUSTER"; then
  log "deleting existing cluster $CLUSTER (--fresh)"
  kind delete cluster --name "$CLUSTER" || exit 1
fi
if ! kind get clusters 2>/dev/null | grep -qx "$CLUSTER"; then
  log "creating kind cluster $CLUSTER"
  kind create cluster --name "$CLUSTER" --config deploy/kind/kind-config.yaml --wait 180s || exit 1
fi
k wait --for=condition=Ready node --all --timeout=180s || exit 1
k -n kube-system rollout status ds/kindnet --timeout=120s >/dev/null || exit 1

# ---- 2. NetworkPolicy enforcement proof (negative control) ---------------------------------
# kindnet's network-policy support must really drop traffic: a client reaches a server before a
# default-deny policy and not after. Without this the SEC-02 probes below would prove nothing.
log "NetworkPolicy enforcement check"
NPNS=aurora-e2e-npcheck
k delete ns "$NPNS" --ignore-not-found --wait=true >/dev/null 2>&1
k create ns "$NPNS" >/dev/null || exit 1
k apply -n "$NPNS" -f - >/dev/null <<'EOF' || exit 1
apiVersion: v1
kind: Pod
metadata: {name: srv, labels: {app: srv}}
spec:
  containers: [{name: s, image: registry.k8s.io/e2e-test-images/agnhost:2.53, args: [netexec, --http-port=8080]}]
---
apiVersion: v1
kind: Pod
metadata: {name: cli, labels: {app: cli}}
spec:
  containers: [{name: c, image: registry.k8s.io/e2e-test-images/agnhost:2.53, args: [pause]}]
EOF
k -n "$NPNS" wait --for=condition=Ready pod --all --timeout=180s >/dev/null || exit 1
SRV_IP=$(k -n "$NPNS" get pod srv -o jsonpath='{.status.podIP}')
probe() { k -n "$NPNS" exec cli -- curl -s -m 3 -o /dev/null "http://$SRV_IP:8080/hostname"; }
probe || { echo "control probe failed before any policy" >&2; exit 1; }
k apply -n "$NPNS" -f - >/dev/null <<'EOF'
apiVersion: networking.k8s.io/v1
kind: NetworkPolicy
metadata: {name: deny}
spec: {podSelector: {}, policyTypes: [Ingress, Egress]}
EOF
sleep 5
if probe; then
  echo "NetworkPolicy is NOT enforced by this cluster's CNI (kindnet). Use Calico: see docs/operator/install-helm.md (Local kind cluster)." >&2
  exit 1
fi
log "NetworkPolicy enforced: connection allowed before the default-deny policy, blocked after"
k delete ns "$NPNS" --wait=false >/dev/null 2>&1

# ---- 3. images ----------------------------------------------------------------------------
# OPS-023: the dev build path of OPS-017. kind-dev.sh is sourced in a subshell (its variables must not leak into
# this script) and its own build_rust / thin_image do the work: cargo in the builder container against the
# bind-mounted source with a persistent target volume, binaries copied into thin distroless images. Images are
# tagged $REGISTRY/aurora-<svc>:$TAG, like the release build's, so everything below is unchanged.
dev_rust_images() (
  e2e_reg=$REGISTRY; e2e_tag=$TAG
  AURORA_DEV_SOURCED=1 . deploy/dev/kind-dev.sh
  REG=$e2e_reg; TAG=$e2e_tag; WORK="$ROOT/target/kind/dev"   # own sha bookkeeping: not the dev cluster's
  # (P4-PRV-2) previews on: the relay service binary joins the package set and gets a thin image.
  [ -n "${PREVIEWS_DOMAIN:-}" ] && RUST_SVCS+=(relay)
  CHANGED_SVCS=()
  build_rust || exit 1
  THINS=(api gateway hookhost runner connector-host)
  [ -n "${PREVIEWS_DOMAIN:-}" ] && THINS+=(relay)
  for s in "${THINS[@]}"; do
    thin_image "$s" || exit 1
    [ -f "$WORK/bin/$s.sha.pending" ] && mv -f "$WORK/bin/$s.sha.pending" "$WORK/bin/$s.sha"
    podman image exists "$REG/aurora-$s:$TAG" || { echo "thin image for $s missing" >&2; exit 1; }
  done
  exit 0
)
IMAGES=(aurora-api aurora-gateway aurora-hookhost aurora-runner aurora-connector-host aurora-web aurora-sandbox aurora-dev-idp)
# (P4-PRV-2) the relay image only exists when the release installs one (previews on).
[ -n "$PREVIEWS_DOMAIN" ] && IMAGES+=(aurora-relay)
if [ "$SKIP_BUILD" != "1" ]; then
  if [ "$RELEASE_IMAGES" = "1" ]; then
    TARGETS=(api gateway hookhost runner connector-host)
    [ -n "$PREVIEWS_DOMAIN" ] && TARGETS+=(relay)
    for t in "${TARGETS[@]}"; do
      log "building aurora-$t (release Containerfile)"
      podman build -q -f images/services/Containerfile --target "$t" -t "$REGISTRY/aurora-$t:$TAG" . >/dev/null || exit 1
    done
  else
    dev_rust_images || exit 1
  fi
  log "building aurora-dev-idp (development IdP for OIDC-only sign-in, P1-005)"
  podman build -q -f deploy/dev/dev-idp.Containerfile -t "$REGISTRY/aurora-dev-idp:$TAG" . >/dev/null || exit 1
  log "building aurora-web"
  podman build -q -f images/services/web.Containerfile -t "$REGISTRY/aurora-web:$TAG" . >/dev/null || exit 1
  log "building aurora-sandbox (sandbox-min target, fake harness)"
  scripts/images/build-sandbox-min.sh "$REGISTRY/aurora-sandbox:$TAG" >/dev/null || exit 1
fi
mkdir -p "$WORK/archives"
# The sandbox image is `runner.image` (deploy/kind/values-e2e.yaml: $REGISTRY/aurora-sandbox:$TAG), the
# single source of truth the runner applies and the API reports; it is loaded under that name.
for i in "${IMAGES[@]}"; do
  ref="$REGISTRY/$i:$TAG"
  podman image exists "$ref" || { echo "missing image $ref" >&2; exit 1; }
  log "loading $i into kind as $ref"
  # `podman save -o` refuses an existing file; an interrupted earlier run can leave one behind.
  rm -f "$WORK/archives/$i.tar"
  podman save -o "$WORK/archives/$i.tar" "$ref" >/dev/null || exit 1
  kind load image-archive "$WORK/archives/$i.tar" --name "$CLUSTER" >/dev/null || exit 1
  rm -f "$WORK/archives/$i.tar"
done

# ---- 4. Helm install (docs/operator/install-helm.md) ----------------------------------------------
log "helm lint / template"
helm lint deploy/helm/aurora -f deploy/kind/values-e2e.yaml "${HELM_OVERRIDES[@]}" >/dev/null || exit 1
helm template "$RELEASE" deploy/helm/aurora -n "$NS" -f deploy/kind/values-e2e.yaml "${HELM_OVERRIDES[@]}" >"$WORK/rendered.yaml" || exit 1
if helm --kube-context "$CTX" status "$RELEASE" -n "$NS" >/dev/null 2>&1; then
  log "removing previous release"
  helm --kube-context "$CTX" uninstall "$RELEASE" -n "$NS" --wait >/dev/null || exit 1
fi
# The evaluation Postgres and the generated Secrets are not removed by `helm uninstall` (docs: Database).
k delete ns "$NS" "$SBX_NS" --ignore-not-found --wait=true >/dev/null || exit 1
# FIX-016: the DBA's steps (docs/operator/install-helm.md, Database), so the gateway runs as its
# least-privilege role. The evaluation Postgres creates the role from an init script on its empty
# data directory; the gateway Secret logs in as it. (Evaluation password: not a secret.)
log "creating the gateway role init script and the gateway database Secret"
k create ns "$NS" >/dev/null || exit 1
k -n "$NS" create configmap aurora-e2e-pg-init \
  --from-literal=10-gateway-role.sql="CREATE ROLE aurora_gateway LOGIN PASSWORD 'gwe2e-evaluation-pw';" >/dev/null || exit 1
k -n "$NS" create secret generic aurora-gateway-database \
  --from-literal=AURORA_DATABASE_URL="postgres://aurora_gateway:gwe2e-evaluation-pw@$FULL-postgres:5432/aurora" >/dev/null || exit 1
log "helm install"
helm --kube-context "$CTX" install "$RELEASE" deploy/helm/aurora -n "$NS" --create-namespace \
  -f deploy/kind/values-e2e.yaml "${HELM_OVERRIDES[@]}" --wait --timeout 8m || exit 1
k -n "$NS" get pods -o wide >&2

# (FIX-102) The runner's Kubernetes API egress is exactly the API server address(es) the chart read from the
# `kubernetes` EndpointSlice in `default` (a /32 or /128 each), never 0.0.0.0/0. `--wait` above already proves the
# httpGet /readyz and /healthz probes (kubelet -> pod, including the gateway's policy-restricted 4103) pass.
log "asserting the runner policy carries the API server address from the EndpointSlice"
SLICE_ADDRS="$(k -n default get endpointslice kubernetes -o jsonpath='{range .endpoints[*]}{.addresses[*]}{"\n"}{end}' 2>&1)" \
  || { echo "cannot read endpointslice default/kubernetes ($SLICE_ADDRS): the chart's lookup needs it" >&2; exit 1; }
RUNNER_CIDRS="$(k -n "$NS" get networkpolicy "$FULL-runner" -o jsonpath='{range .spec.egress[*].to[*]}{.ipBlock.cidr}{"\n"}{end}')"
echo "  EndpointSlice addresses: $(printf '%s' "$SLICE_ADDRS" | tr '\n' ' ')" >&2
echo "  runner policy ipBlocks:  $(printf '%s' "$RUNNER_CIDRS" | tr '\n' ' ')" >&2
[ -n "$SLICE_ADDRS" ] || { echo "the kubernetes EndpointSlice has no addresses" >&2; exit 1; }
if printf '%s\n' "$RUNNER_CIDRS" | grep -qx '0.0.0.0/0'; then echo "runner policy opens 0.0.0.0/0" >&2; exit 1; fi
for a in $SLICE_ADDRS; do
  case "$a" in *:*) want="$a/128" ;; *) want="$a/32" ;; esac
  printf '%s\n' "$RUNNER_CIDRS" | grep -qx "$want" || { echo "runner policy lacks the API server address $want" >&2; exit 1; }
done
# No pod may sit un-Ready behind a TCP-only check any more: every Deployment with an ops router uses httpGet.
for d in api gateway hookhost runner connector-host; do
  p="$(k -n "$NS" get "deploy/$FULL-$d" -o jsonpath='{.spec.template.spec.containers[0].readinessProbe.httpGet.path}' 2>/dev/null)"
  [ "$p" = "/readyz" ] || { echo "deploy/$FULL-$d readiness probe is not httpGet /readyz (got '$p')" >&2; exit 1; }
done
echo "  readiness probes are httpGet /readyz; PodDisruptionBudgets: $(k -n "$NS" get pdb -o name 2>/dev/null | tr '\n' ' ')" >&2

# ---- 5. reach the API, run the scenarios ---------------------------------------------------------
log "port-forward svc/$FULL-api -> 127.0.0.1:$LOCAL_PORT"
k -n "$NS" port-forward "svc/$FULL-api" "$LOCAL_PORT:4101" >"$WORK/port-forward.log" 2>&1 &
PF_PID=$!
for _ in $(seq 60); do
  curl -fsS "http://127.0.0.1:$LOCAL_PORT/readyz" >/dev/null 2>&1 && break
  kill -0 "$PF_PID" 2>/dev/null || { cat "$WORK/port-forward.log" >&2; exit 1; }
  sleep 1
done
curl -fsS "http://127.0.0.1:$LOCAL_PORT/readyz" >/dev/null || { echo "api not ready via port-forward" >&2; exit 1; }
# P1-005 (RFE-004): sign-in is OIDC only; the dev IdP (values-e2e.yaml: devIdp) is reached at the issuer URL
# the API hands out, http://localhost:$IDP_PORT (4107 by default), so forward that port too.
log "port-forward svc/$FULL-dev-idp -> 127.0.0.1:$IDP_PORT"
k -n "$NS" port-forward "svc/$FULL-dev-idp" "$IDP_PORT:4107" >"$WORK/port-forward-idp.log" 2>&1 &
IDP_PF_PID=$!
for _ in $(seq 30); do
  curl -fsS "http://localhost:$IDP_PORT/.well-known/openid-configuration" >/dev/null 2>&1 && break
  kill -0 "$IDP_PF_PID" 2>/dev/null || { cat "$WORK/port-forward-idp.log" >&2; exit 1; }
  sleep 1
done
curl -fsS "http://localhost:$IDP_PORT/.well-known/openid-configuration" >/dev/null || { echo "dev IdP not reachable via port-forward" >&2; exit 1; }

# (P4-PRV-2) previews on: forward the relay's edge too. The preview DNS suffix is synthetic
# (nothing resolves it), so the tests address the relay directly with an explicit Host header.
if [ -n "$PREVIEWS_DOMAIN" ]; then
  RELAY_PORT="${AURORA_E2E_RELAY_PORT:-14102}"
  log "port-forward svc/$FULL-relay -> 127.0.0.1:$RELAY_PORT"
  k -n "$NS" port-forward "svc/$FULL-relay" "$RELAY_PORT:4112" >"$WORK/port-forward-relay.log" 2>&1 &
  RELAY_PF_PID=$!
  for _ in $(seq 60); do
    # Any HTTP answer (even the 404 for a never-created label) proves the edge is listening.
    curl -s -m 3 -o /dev/null -H "Host: x.$PREVIEWS_DOMAIN" "http://127.0.0.1:$RELAY_PORT/" >/dev/null 2>&1 && break
    kill -0 "$RELAY_PF_PID" 2>/dev/null || { cat "$WORK/port-forward-relay.log" >&2; exit 1; }
    sleep 1
  done
  curl -s -m 3 -o /dev/null -H "Host: x.$PREVIEWS_DOMAIN" "http://127.0.0.1:$RELAY_PORT/" || { echo "relay not reachable via port-forward" >&2; exit 1; }
fi

log "running tests/e2e-hosted/tests/k8s.rs"
# (P4-PRV-2) previews on: the relay edge base and the synthetic suffix for the k6 round trip
# (an env-assignment prefix cannot be built with ${VAR:+...} — it would expand as a command).
if [ -n "$PREVIEWS_DOMAIN" ]; then
  export AURORA_KIND_PREVIEWS_DOMAIN="$PREVIEWS_DOMAIN"
  export AURORA_KIND_RELAY_URL="http://127.0.0.1:$RELAY_PORT"
fi
AURORA_TEST_KIND=1 \
AURORA_KIND_API="http://127.0.0.1:$LOCAL_PORT" \
AURORA_KIND_ORIGIN="http://localhost:4100" \
AURORA_KIND_CONTEXT="$CTX" AURORA_KIND_NS="$NS" AURORA_KIND_SBX_NS="$SBX_NS" AURORA_KIND_FULL="$FULL" \
AURORA_KIND_PG_POD="$FULL-postgres-0" \
  timeout 1500 cargo test -p aurora-e2e-hosted --test k8s -- --test-threads=1 --nocapture "${CARGO_ARGS[@]}"
STATUS=$?
if [ "$STATUS" -eq 0 ]; then log "kind e2e PASSED"; else log "kind e2e FAILED (rc=$STATUS)"; fi
[ "$STATUS" -eq 0 ] || exit "$STATUS"

# ---- 6. Helm upgrade legs (OPS-024) ---------------------------------------------------------------
# Value-only `helm upgrade` legs on the release the tests just ran against: the hooks run before
# any regular resource under the policies the previous release left in place, so a hook that
# cannot reach what it needs (the 2026-11-11 incident: unlabeled hook pod, no allow policy,
# `kubectl` hung) times out here instead of in production. The images are already loaded; each
# leg adds ~1–2 minutes. `AURORA_E2E_UPGRADE_LEGS=0` (or --no-upgrade-legs) keeps the old
# fresh-install-only flow.
upgrade_leg() {
  local label="$1" replicas="$2" err
  err="$WORK/upgrade-$(printf '%s' "$label" | tr -c 'A-Za-z0-9' _).err"
  log "helm upgrade leg: $label (web.replicas=$replicas)"
  if ! timeout 300 helm --kube-context "$CTX" upgrade "$RELEASE" deploy/helm/aurora -n "$NS" \
      -f deploy/kind/values-e2e.yaml "${HELM_OVERRIDES[@]}" --set "web.replicas=$replicas" \
      --wait --timeout 4m 2>"$err"; then
    cat "$err" >&2
    echo "helm upgrade leg '$label' FAILED:" >&2
    echo "  hook jobs/pods (look for the hook timeout: BackoffLimitExceeded, hangs in kubectl):" >&2
    k -n "$NS" get jobs,pods -l "job-name=$FULL-secrets" -o wide >&2 || true
    echo "  hook pod logs:" >&2
    for p in $(k -n "$NS" get pods -l "job-name=$FULL-secrets" -o name 2>/dev/null); do
      k -n "$NS" logs "$p" --all-containers --tail=100 >&2 || true
    done
    echo "  networkpolicies in $NS:" >&2
    k -n "$NS" get networkpolicy -o name >&2 || true
    return 1
  fi
  cat "$err" >&2
  # (FIX-102) The chart reads the API server addresses from the discovery.k8s.io/v1 EndpointSlice; a read of the
  # core v1 Endpoints made every upgrade on Kubernetes >= 1.33 warn "v1 Endpoints is deprecated".
  if grep -qi 'is deprecated' "$err"; then
    echo "helm upgrade leg '$label': helm printed a deprecation warning:" >&2
    grep -i 'is deprecated' "$err" >&2
    return 1
  fi
  local st
  st=$(helm --kube-context "$CTX" status "$RELEASE" -n "$NS" -o json | python3 -I -c 'import json,sys; print(json.load(sys.stdin)["info"]["status"])')
  [ "$st" = "deployed" ] || { echo "helm upgrade leg '$label': release status is $st, expected deployed" >&2; return 1; }
  k -n "$NS" rollout status "deploy/$FULL-web" --timeout=120s >/dev/null \
    || { echo "helm upgrade leg '$label': $FULL-web did not roll (replicas=$replicas)" >&2; return 1; }
  local ready
  ready=$(k -n "$NS" get "deploy/$FULL-web" -o jsonpath='{.status.readyReplicas}')
  [ "$ready" = "$replicas" ] || { echo "helm upgrade leg '$label': $FULL-web has $ready ready replicas, expected $replicas" >&2; return 1; }
  log "helm upgrade leg '$label' ok: release status $st, web replicas=$replicas Ready"
}
if [ "$UPGRADE_LEGS" = "1" ]; then
  # A leg failure sets STATUS so the EXIT trap collects the cluster logs (collect_logs), like a
  # test failure would.
  upgrade_leg "scale up" 2 || { STATUS=1; log "kind e2e FAILED (helm upgrade leg 'scale up')"; exit 1; }
  upgrade_leg "scale back" 1 || { STATUS=1; log "kind e2e FAILED (helm upgrade leg 'scale back')"; exit 1; }
else
  log "skipping the helm upgrade legs (AURORA_E2E_UPGRADE_LEGS=0)"
fi

log "kind e2e PASSED (incl. helm upgrade legs)"
exit 0
