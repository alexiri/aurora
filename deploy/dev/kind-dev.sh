#!/usr/bin/env bash
# OPS-017: the one containerized dev/test setup: a kind cluster + the Helm chart (replaces podman compose).
#
#   deploy/dev/kind-dev.sh up [--llm-mock] [--connector-fake] [--no-sandbox]   cluster + ingress + images + helm release
#   deploy/dev/kind-dev.sh dev [api|gateway|hookhost|runner|connector-host|connector-fake|dev-idp|web|sandbox ...]
#                                          rebuild changed service images, load them, restart those Deployments
#   deploy/dev/kind-dev.sh bootstrap-admin <email>   pre-provision the first instance admin (binds at their first sign-in)
#   deploy/dev/kind-dev.sh status | down
#
# Cluster `aurora-dev` (rootless podman provider), release `aurora` in namespace `aurora`, one URL:
# http://localhost:4000 (ingress-nginx on the node's hostPort 80). Images: localhost/aurora-dev/aurora-<svc>:dev.
# Rust services are built in a builder container (deploy/dev/builder.Containerfile) against the bind-mounted
# source with a persistent target volume `<lane>-kind-dev-target`, so a warm rebuild compiles only the changed
# crates. Binaries are copied into thin distroless images (deploy/dev/thin.Containerfile). A service whose
# binary is byte-identical to the last loaded one is neither re-imaged nor restarted.
# Everything created is named after the worktree basename (lane) or `aurora-dev`; volumes carry label aurora.lane=<lane>.
# Env: AURORA_DEV_CLUSTER (aurora-dev), AURORA_DEV_PORT (4000; the host port of the single URL. Another value is
# written into a generated kind config and into `externalUrl`, so a second cluster can run beside the owner's).
# With --llm-mock the hosted agents run the REAL claude-code harness (P2-E2E-1): the sandbox image is
# scripts/e2e/llm.Containerfile on top of the dev sandbox image, loaded as aurora-sandbox-llm and set as runner.image.
set -uo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

export KIND_EXPERIMENTAL_PROVIDER=podman
CLUSTER="${AURORA_DEV_CLUSTER:-aurora-dev}"
CTX="kind-$CLUSTER"
PORT="${AURORA_DEV_PORT:-4000}"
NS=aurora
RELEASE=aurora
REG=localhost/aurora-dev
TAG=dev
LANE="$(basename "$ROOT")"
VOL_TARGET="$LANE-kind-dev-target"
VOL_REGISTRY="$LANE-kind-dev-cargo-registry"
VOL_RUSTUP="$LANE-kind-dev-rustup"   # `stable` is auto-installed once, then kept
BUILDER_IMAGE="$REG/builder:1"
WORK="$ROOT/target/kind-dev"
RUST_SVCS=(api gateway hookhost runner connector-host connector-fake dev-idp)

k() { kubectl --context "$CTX" "$@"; }
log() { echo "==> $*" >&2; }
die() { echo "kind-dev: $*" >&2; exit 1; }

need() { for t in "$@"; do command -v "$t" >/dev/null || die "missing tool: $t"; done; }

cluster_exists() { kind get clusters 2>/dev/null | grep -qx "$CLUSTER"; }

load_image() { # ref
  mkdir -p "$WORK/archives"
  local tar="$WORK/archives/$(echo "$1" | tr '/:' '__').tar"
  # `podman save -o` refuses an existing file ("docker-archive doesn't support modifying existing
  # images"), and an interrupted earlier run can leave one behind.
  rm -f "$tar"
  podman save -o "$tar" "$1" >/dev/null && kind load image-archive "$tar" --name "$CLUSTER" >/dev/null
  local rc=$?; rm -f "$tar"; return $rc
}

# Builds every Rust service binary (cargo skips unchanged work) into $WORK/bin.
build_rust() {
  podman image exists "$BUILDER_IMAGE" || { log "building $BUILDER_IMAGE"; podman build -q -f deploy/dev/builder.Containerfile -t "$BUILDER_IMAGE" deploy/dev >/dev/null || return 1; }
  mkdir -p "$WORK/bin"
  podman volume exists "$VOL_TARGET" || podman volume create --label "aurora.lane=$LANE" "$VOL_TARGET" >/dev/null
  podman volume exists "$VOL_RUSTUP" || podman volume create --label "aurora.lane=$LANE" "$VOL_RUSTUP" >/dev/null
  podman volume exists "$VOL_REGISTRY" || podman volume create --label "aurora.lane=$LANE" "$VOL_REGISTRY" >/dev/null
  # Always the same package set: a subset would unify cargo features differently and rebuild shared dependencies.
  local pkgs=() s
  # `dev-idp` (P1-005, RFE-004) is the `aurora-dev-idp` binary of the aurora-testkit package.
  for s in "${RUST_SVCS[@]}"; do case "$s" in dev-idp) pkgs+=(-p aurora-testkit) ;; *) pkgs+=(-p "aurora-$s") ;; esac; done
  log "cargo build ${pkgs[*]} (in container, target volume $VOL_TARGET)"
  # .cargo/config.toml is masked: it is for host builds (the image builds ignore it, too).
  podman run --rm --name "$LANE-kind-dev-build" --label "aurora.lane=$LANE" --security-opt label=disable \
    -v "$ROOT:/src:ro" -v /dev/null:/src/.cargo/config.toml:ro \
    -v "$VOL_TARGET:/target" -v "$VOL_REGISTRY:/usr/local/cargo/registry" -v "$VOL_RUSTUP:/usr/local/rustup" -v "$WORK/bin:/out" \
    "$BUILDER_IMAGE" sh -ec "cargo build --locked ${pkgs[*]} && for s in ${RUST_SVCS[*]}; do cp -f /target/debug/aurora-\$s /out/aurora-\$s.new; done" || return 1
}

# image_for <svc>: thin image from the binary; prints nothing, returns 0 and sets CHANGED=1 when it was (re)built.
thin_image() { # svc
  local s=$1 bin="$WORK/bin/aurora-$1" new="$WORK/bin/aurora-$1.new"
  [ -f "$new" ] || return 0
  local h; h=$(sha256sum "$new" | cut -c1-64)
  if [ "$h" = "$(cat "$WORK/bin/$s.sha" 2>/dev/null)" ] && podman image exists "$REG/aurora-$s:$TAG"; then rm -f "$new"; return 0; fi
  mv -f "$new" "$bin"
  local ctx; ctx=$(mktemp -d "$WORK/ctx.XXXXXX"); cp "$bin" "$ctx/aurora-$s"
  podman build -q -f deploy/dev/thin.Containerfile --build-arg "BIN=aurora-$s" -t "$REG/aurora-$s:$TAG" "$ctx" >/dev/null || { rm -rf "$ctx"; return 1; }
  rm -rf "$ctx"
  echo "$h" >"$WORK/bin/$s.sha.pending"
  CHANGED_SVCS+=("$s")
}

web_hash() { git ls-files -z --cached --others --exclude-standard -- web images/services/web.Containerfile | xargs -0 sha256sum 2>/dev/null | sha256sum | cut -c1-64; }

build_web() { # prints via CHANGED_SVCS
  local h; h=$(web_hash)
  if [ "$h" = "$(cat "$WORK/web.sha" 2>/dev/null)" ] && podman image exists "$REG/aurora-web:$TAG"; then return 0; fi
  log "building aurora-web"
  podman build -q -f images/services/web.Containerfile -t "$REG/aurora-web:$TAG" . >/dev/null || return 1
  echo "$h" >"$WORK/web.sha.pending"; CHANGED_SVCS+=(web)
}

build_sandbox() {
  log "building aurora-sandbox (sandbox-min target, fake harness)"
  scripts/images/build-sandbox-min.sh "$REG/aurora-sandbox:$TAG" >/dev/null || return 1
  CHANGED_SVCS+=(sandbox)
}

# The --llm-mock profile's sandbox image: the dev sandbox plus the pinned Claude Code CLI (FU-OPS-017a).
# Loaded only when its image id changed (the npm layer is cached by podman, so a rebuild is cheap).
SANDBOX_LLM_IMAGE="$REG/aurora-sandbox-llm:$TAG"
build_sandbox_llm() {
  podman image exists "$REG/aurora-sandbox:$TAG" || { log "no $REG/aurora-sandbox:$TAG: build the sandbox first (kind-dev.sh dev sandbox)"; return 1; }
  log "building aurora-sandbox-llm (scripts/e2e/llm.Containerfile: claude-code harness)"
  podman build -q -f scripts/e2e/llm.Containerfile --build-arg "BASE=$REG/aurora-sandbox:$TAG" -t "$SANDBOX_LLM_IMAGE" . >/dev/null || return 1
  mkdir -p "$WORK"
  local id; id=$(podman image inspect --format '{{.Id}}' "$SANDBOX_LLM_IMAGE")
  if [ "$id" = "$(cat "$WORK/sandbox-llm.sha" 2>/dev/null)" ]; then return 0; fi
  log "loading aurora-sandbox-llm into $CLUSTER"
  load_image "$SANDBOX_LLM_IMAGE" || return 1
  echo "$id" >"$WORK/sandbox-llm.sha"
}

# kind config with the published host port (deploy/kind/kind-config-dev.yaml is the :4000 template).
kind_config() {
  mkdir -p "$WORK"
  [ "$PORT" = 4000 ] && { echo "deploy/kind/kind-config-dev.yaml"; return 0; }
  local out="$WORK/kind-config-dev-$PORT.yaml" body
  body=$(<deploy/kind/kind-config-dev.yaml)
  printf '%s\n' "${body//hostPort: 4000/hostPort: $PORT}" >"$out"
  echo "$out"
}

# Build what changed among $@ (service names); load changed images into the cluster; mark them current.
build_and_load() {
  CHANGED_SVCS=()
  mkdir -p "$WORK"
  local rust=() s
  for s in "$@"; do case "$s" in web|sandbox) ;; *) rust+=("$s") ;; esac; done
  if [ "${#rust[@]}" -gt 0 ]; then
    build_rust "${rust[@]}" || return 1
    for s in "${rust[@]}"; do thin_image "$s" || return 1; done
  fi
  for s in "$@"; do
    case "$s" in
      web) build_web || return 1 ;;
      sandbox) build_sandbox || return 1 ;;
    esac
  done
  for s in "${CHANGED_SVCS[@]}"; do
    log "loading aurora-$s into $CLUSTER"
    load_image "$REG/aurora-$s:$TAG" || return 1
    for f in "$WORK/bin/$s.sha" "$WORK/web.sha"; do [ -f "$f.pending" ] && mv -f "$f.pending" "$f"; done
  done
  return 0
}

ensure_cluster() {
  if ! cluster_exists; then
    log "creating kind cluster $CLUSTER"
    kind create cluster --name "$CLUSTER" --config "$(kind_config)" --wait 180s >&2 || die "kind create failed"
    rm -f "$WORK"/bin/*.sha "$WORK/web.sha" "$WORK/sandbox-llm.sha"   # a new cluster has none of the images loaded
  fi
  k wait --for=condition=Ready node --all --timeout=180s >/dev/null || die "node not ready"
  k -n kube-system rollout status ds/kindnet --timeout=120s >/dev/null || die "kindnet not ready"
  if ! k -n ingress-nginx get deploy ingress-nginx-controller >/dev/null 2>&1; then
    log "installing ingress-nginx (deploy/kind/ingress-nginx.yaml)"
    k apply -f deploy/kind/ingress-nginx.yaml >/dev/null || die "ingress-nginx apply failed"
  fi
  k -n ingress-nginx rollout status deploy/ingress-nginx-controller --timeout=240s >/dev/null || die "ingress-nginx not ready"
}

cmd_up() {
  local mock=0 fake=0 sandbox=1
  while [ $# -gt 0 ]; do
    case "$1" in
      --llm-mock) mock=1 ;; --connector-fake) fake=1 ;; --no-sandbox) sandbox=0 ;;
      *) die "unknown argument: $1" ;;
    esac; shift
  done
  need kind kubectl helm podman
  local free_kb; free_kb=$(df --output=avail -k . | tail -1)
  [ "$free_kb" -ge $((15 * 1024 * 1024)) ] || die "less than 15 GB free; stopping"
  ensure_cluster
  # dev-idp: the development IdP (P1-005, RFE-004; values-dev.yaml devIdp). All sign-in is OIDC.
  local svcs=(api gateway hookhost runner connector-host web dev-idp) vals=(-f deploy/kind/values-dev.yaml)
  [ "$fake" = 1 ] && { svcs+=(connector-fake); vals+=(-f deploy/kind/values-connector-fake.yaml); }
  [ "$sandbox" = 1 ] && svcs+=(sandbox)
  build_and_load "${svcs[@]}" || die "image build/load failed"
  if [ "$mock" = 1 ]; then
    vals+=(-f deploy/kind/values-llm-mock.yaml)
    [ "$sandbox" = 1 ] || die "--llm-mock needs the sandbox image (the claude-code harness image is built on it): drop --no-sandbox"
    build_sandbox_llm || die "aurora-sandbox-llm build/load failed"
    log "building and loading vidaimock (config baked in)"
    podman build -q -f deploy/dev/llm-mock/vidaimock.Containerfile -t "$REG/vidaimock:$TAG" deploy/dev/llm-mock >/dev/null || die "vidaimock build failed"
    load_image "$REG/vidaimock:$TAG" || die "vidaimock load failed"
    k apply -f - >/dev/null <<<'{"apiVersion":"v1","kind":"Namespace","metadata":{"name":"aurora-dev-mock"}}'
    k -n aurora-dev-mock create configmap litellm-config --from-file=litellm.yaml=deploy/dev/llm-mock/litellm.yaml \
      --dry-run=client -o yaml | k apply -f - >/dev/null || die "litellm configmap failed"
    k apply -f deploy/dev/llm-mock/llm-mock.yaml >/dev/null || die "llm-mock apply failed"
  fi
  log "helm upgrade --install $RELEASE (${vals[*]})"
  helm --kube-context "$CTX" upgrade --install "$RELEASE" deploy/helm/aurora -n "$NS" --create-namespace \
    "${vals[@]}" --set "externalUrl=http://localhost:$PORT" --wait --timeout 8m >&2 || die "helm install failed (kubectl --context $CTX -n $NS get pods)"
  if [ "$mock" = 1 ]; then
    k -n aurora-dev-mock rollout status deploy/vidaimock deploy/litellm --timeout=300s >/dev/null 2>&1 \
      || k -n aurora-dev-mock rollout status deploy/litellm --timeout=300s >/dev/null || die "llm-mock not ready"
  fi
  k -n "$NS" get pods >&2
  log "Aurora is up: http://localhost:$PORT  (sign in with 'Development sign-in' and pick a user; kubectl --context $CTX -n $NS ...)"
  log "first instance admin: deploy/dev/kind-dev.sh bootstrap-admin you@example.com  (then sign in with that e-mail)"
  [ "$mock" = 1 ] && log "LLM endpoint litellm-mock, key sk-aurora-mock-dev; hosted agents run the real claude-code harness (runner.image $SANDBOX_LLM_IMAGE)"
  return 0
}

cmd_dev() {
  need kind kubectl podman
  cluster_exists || die "cluster $CLUSTER does not exist: run 'deploy/dev/kind-dev.sh up' first"
  local svcs=("$@")
  [ "${#svcs[@]}" -gt 0 ] || svcs=(api gateway hookhost runner connector-host web dev-idp)
  local s; for s in "${svcs[@]}"; do
    case "$s" in api|gateway|hookhost|runner|connector-host|connector-fake|dev-idp|web|sandbox) ;; *) die "unknown service: $s" ;; esac
  done
  local t0=$SECONDS
  build_and_load "${svcs[@]}" || die "image build/load failed"
  local restarted=()
  for s in "${CHANGED_SVCS[@]}"; do
    [ "$s" = sandbox ] && continue   # picked up by the next sandbox Pod
    local dep="$RELEASE-$s"
    k -n "$NS" get deploy "$dep" >/dev/null 2>&1 || { log "no deployment $dep (profile not installed?); skipped"; continue; }
    log "restarting $dep"
    k -n "$NS" rollout restart "deploy/$dep" >/dev/null || die "restart $dep failed"
    restarted+=("$dep")
  done
  # A cluster with the llm-mock profile runs the harness image, which is built on the sandbox image.
  if [[ " ${CHANGED_SVCS[*]} " == *" sandbox "* ]] && k get ns aurora-dev-mock >/dev/null 2>&1; then
    build_sandbox_llm || die "aurora-sandbox-llm build/load failed"
  fi
  for d in "${restarted[@]}"; do k -n "$NS" rollout status "deploy/$d" --timeout=180s >/dev/null || die "$d did not become ready"; done
  log "done in $((SECONDS - t0))s: rebuilt+restarted: ${CHANGED_SVCS[*]:-nothing (all images current)}"
}

# bootstrap-admin <email>: pre-provision an instance admin in the dev cluster. In the dev images the binary is
# /usr/local/bin/aurora-service (deploy/dev/thin.Containerfile), not /usr/local/bin/aurora-api.
cmd_bootstrap_admin() {
  need kubectl
  [ -n "${1:-}" ] || die "usage: kind-dev.sh bootstrap-admin <email>"
  k -n "$NS" exec "deploy/$RELEASE-api" -- /usr/local/bin/aurora-service bootstrap-admin --email "$1"
}

cmd_status() {
  cluster_exists || { echo "cluster $CLUSTER: absent"; return 0; }
  k -n "$NS" get pods -o wide
  k -n aurora-dev-mock get pods 2>/dev/null || true
}

cmd_down() {
  need kind
  cluster_exists && kind delete cluster --name "$CLUSTER" >&2
  podman volume exists "$VOL_TARGET" 2>/dev/null && log "kept cargo cache volume $VOL_TARGET (remove: podman volume rm $VOL_TARGET $VOL_REGISTRY $VOL_RUSTUP)"
  rm -rf "$WORK/bin"/*.sha "$WORK/web.sha"
  return 0
}

# OPS-023: scripts/e2e/kind.sh sources this file for build_rust/thin_image (AURORA_DEV_SOURCED=1): no dispatch then.
if [ -z "${AURORA_DEV_SOURCED:-}" ]; then
case "${1:-}" in
  up) shift; cmd_up "$@" ;;
  dev) shift; cmd_dev "$@" ;;
  bootstrap-admin) shift; cmd_bootstrap_admin "$@" ;;
  status) cmd_status ;;
  down) cmd_down ;;
  *) sed -n '2,12p' "$0" | sed 's/^# \{0,1\}//'; exit 2 ;;
esac
fi
