#!/usr/bin/env bash
# M4 acceptance: real aurora-api + aurora-gateway + aurora-runner (podman) + sandbox image (fake harness)
# (TEST-013). Gated by AURORA_TEST_PODMAN=1.
#
# Networking (rootless podman): the bridge of an internal podman network lives in podman's
# rootless network namespace, not in the host's. So the whole suite (test process, api, gateway,
# runner, mock upstream) runs inside that namespace via `podman unshare --rootless-netns`, bound on the
# bridge IP 10.89.251.1 (gateway :4102/:4103, runner :4105/:4106) which sandboxes reach through
# /etc/hosts entries aurora-gateway/aurora-runner. Postgres runs as a container on a separate network
# (10.89.252.5), unreachable from the sandbox network. An "anchor" container keeps the bridge up.
# Env: SKIP_IMAGE_BUILD=1 reuses the existing localhost/aurora-sandbox:0.1.0.
# Runs only the hosted test target. The channel tests belong to scripts/e2e/channels.sh; the k8s and
# llm targets are added only when their gates are set (AURORA_TEST_KIND=1, AURORA_TEST_LLM=1).
set -uo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"
if [ "${AURORA_TEST_PODMAN:-}" != "1" ]; then echo "skipped: set AURORA_TEST_PODMAN=1" >&2; exit 0; fi

# Isolation (P2-TEST-1, FU-P2-TEST-hosted). Everything a run creates is named from one prefix and
# placed on addresses that can be overridden, so two runs (two worktrees, or a worktree and master)
# do not collide. The defaults are the historical values, so a plain run is unchanged:
#   AURORA_E2EH_PREFIX    names: <prefix>-sbx/-db networks, -pg/-dbtarget/-anchor containers,
#                         <prefix>-sandbox-full image; also salts the installation token, so the
#                         sandbox label (`aurora.installation=<fp>`) and the cleanup differ per run.
#   AURORA_E2EH_SBX_BASE  first three octets of the sandbox network /24 (default 10.89.251; bridge .1)
#   AURORA_E2EH_DB_BASE   the same for the database network (default 10.89.252; pg .5, dbtarget .6)
#   AURORA_E2EH_API_IP / AURORA_E2EH_OPS_IP  loopback addresses in the shared rootless netns
#                         (defaults 127.0.0.41 / 127.0.0.42; ports are fixed, addresses must differ)
# Example for a second run next to a default one:
#   AURORA_E2EH_PREFIX=p2-test-1 AURORA_E2EH_SBX_BASE=10.89.241 AURORA_E2EH_DB_BASE=10.89.242 \
#   AURORA_E2EH_API_IP=127.0.0.51 AURORA_E2EH_OPS_IP=127.0.0.52 AURORA_TEST_PODMAN=1 scripts/e2e/hosted.sh
PREFIX="${AURORA_E2EH_PREFIX:-aurora-e2eh}"
SBX_BASE="${AURORA_E2EH_SBX_BASE:-10.89.251}"
DB_BASE="${AURORA_E2EH_DB_BASE:-10.89.252}"
export AURORA_E2EH_API_IP="${AURORA_E2EH_API_IP:-127.0.0.41}"
export AURORA_E2EH_OPS_IP="${AURORA_E2EH_OPS_IP:-127.0.0.42}"
export AURORA_E2EH_BRIDGE="$SBX_BASE.1"
export AURORA_E2EH_SBX_OTHER_IP="$SBX_BASE.2"
export AURORA_E2EH_PG_IP="$DB_BASE.5"
SBX_NET="$PREFIX-sbx"
DB_NET="$PREFIX-db"
PG="$PREFIX-pg"
# P2-DBL-2: the PostgreSQL that database connections point at. It is not the platform database
# (the guarded dial refuses that by address and port), so the database lane has a server to reach.
DBTARGET="$PREFIX-dbtarget"
ANCHOR="$PREFIX-anchor"
if [ "$PREFIX" = "aurora-e2eh" ]; then
  TOKEN="aur_e2ehostedtoken"
  DEFAULT_IMAGE="localhost/aurora-sandbox:0.1.0"
else
  TOKEN="aur_e2eh$(printf '%s' "$PREFIX" | sha256sum | cut -c1-16)"
  DEFAULT_IMAGE="localhost/$PREFIX-sandbox:0.1.0"
fi
FP=$(printf '%s' "$TOKEN" | sha256sum | cut -c1-16)
# AURORA_E2EH_IMAGE overrides the default tag (shared when the prefix is the default one), e.g. to
# test a supervisor change without replacing the image other runs use.
IMAGE="${AURORA_E2EH_IMAGE:-$DEFAULT_IMAGE}"
# AURORA_E2EH_FULL_IMAGE=1 also builds the full sandbox image (the `browser` capability label,
# Chromium, the pinned Claude Code CLI) for the chromium scenario (SEC-41, FU-P1-003b); without it
# that scenario prints SKIPPED. Measured: ~10 min with a cold supervisor layer (dnf Chromium and
# the Claude Code CLI included; much less when the layers are cached). SANDBOX_BASE_IMAGE=<image> overrides
# the base image as for the min image. SKIP_IMAGE_BUILD=1 reuses an existing $FULL_IMAGE.
FULL_IMAGE="${AURORA_E2EH_FULL_IMAGE_TAG:-localhost/$PREFIX-sandbox-full:0.1.0}"

cleanup() {
  podman ps -a -q --filter "label=aurora.installation=$FP" | xargs -r podman rm -f -t 1 >/dev/null 2>&1
  podman volume ls -q --filter "label=aurora.installation=$FP" | xargs -r podman volume rm -f >/dev/null 2>&1
  podman rm -f -t 1 "$PG" "$DBTARGET" "$ANCHOR" >/dev/null 2>&1
  podman network rm "$SBX_NET" "$DB_NET" >/dev/null 2>&1
}
trap cleanup EXIT
cleanup

free_kb=$(df --output=avail -k . | tail -1)
if [ "$free_kb" -lt $((15 * 1024 * 1024)) ]; then echo "less than 15 GB free; stopping" >&2; exit 1; fi

cargo build -p aurora-api -p aurora-gateway -p aurora-runner || { echo "cargo build failed" >&2; exit 1; }
if [ "${SKIP_IMAGE_BUILD:-}" != "1" ]; then
  scripts/images/build-sandbox-min.sh "${IMAGE#localhost/}" || { echo "image build failed" >&2; exit 1; }
fi
podman image exists "$IMAGE" || { echo "missing $IMAGE" >&2; exit 1; }
if [ "${AURORA_E2EH_FULL_IMAGE:-}" = "1" ]; then
  if [ "${SKIP_IMAGE_BUILD:-}" != "1" ]; then
    base_args=()
    [ -n "${SANDBOX_BASE_IMAGE:-}" ] && base_args=(--build-arg "SANDBOX_BASE_IMAGE=$SANDBOX_BASE_IMAGE")
    podman build -f images/sandbox/Containerfile --target sandbox "${base_args[@]}" \
      -t "$FULL_IMAGE" . || { echo "full image build failed" >&2; exit 1; }
  fi
  podman image exists "$FULL_IMAGE" || { echo "missing $FULL_IMAGE" >&2; exit 1; }
  export AURORA_E2EH_FULL_IMAGE_TAG="$FULL_IMAGE"
fi
TARGETS=(--test hosted)
[ "${AURORA_TEST_KIND:-}" = "1" ] && TARGETS+=(--test k8s)
[ "${AURORA_TEST_LLM:-}" = "1" ] && TARGETS+=(--test llm)
cargo test -p aurora-e2e-hosted "${TARGETS[@]}" --no-run || exit 1

podman network create --internal --disable-dns --subnet "$SBX_BASE.0/24" --gateway "$SBX_BASE.1" "$SBX_NET" >/dev/null || exit 1
podman network create --disable-dns --subnet "$DB_BASE.0/24" --gateway "$DB_BASE.1" "$DB_NET" >/dev/null || exit 1
podman run -d --name "$ANCHOR" --network "$SBX_NET" --entrypoint sleep "$IMAGE" 100000 >/dev/null || exit 1
podman run -d --name "$PG" --network "$DB_NET" --ip "$DB_BASE.5" \
  -e POSTGRES_USER=aurora -e POSTGRES_PASSWORD=aurora -e POSTGRES_DB=postgres \
  docker.io/library/postgres:16 -c max_connections=300 -c fsync=off >/dev/null || exit 1
podman run -d --name "$DBTARGET" --network "$DB_NET" --ip "$DB_BASE.6" \
  -e POSTGRES_USER=aurora -e POSTGRES_PASSWORD=aurora -e POSTGRES_DB=postgres \
  docker.io/library/postgres:16 -c max_connections=50 -c fsync=off >/dev/null || exit 1
for _ in $(seq 60); do
  podman exec "$PG" pg_isready -U aurora >/dev/null 2>&1 \
    && podman exec "$DBTARGET" pg_isready -U aurora >/dev/null 2>&1 && break
  sleep 1
done

SOCK=$(podman info --format '{{.Host.RemoteSocket.Path}}')
# (FIX-101) With CI=true a SKIPPED: line anywhere in the run fails it (scripts/e2e/lib-skip.sh).
# shellcheck source=scripts/e2e/lib-skip.sh
source scripts/e2e/lib-skip.sh
run_logged "$(mktemp /tmp/aurora-e2e-hosted-log.XXXXXX)" podman unshare --rootless-netns env \
  AURORA_TEST_DATABASE_URL="postgres://aurora:aurora@$DB_BASE.5:5432/postgres" \
  AURORA_E2EH_SOCK="$SOCK" AURORA_E2EH_NETWORK="$SBX_NET" AURORA_E2EH_TOKEN="$TOKEN" \
  AURORA_E2EH_FP="$FP" AURORA_E2EH_IMAGE="$IMAGE" AURORA_E2EH_DBTARGET="$DB_BASE.6:5432" \
  cargo test -p aurora-e2e-hosted "${TARGETS[@]}" -- --test-threads=1 --nocapture "$@"
rc=$?
exit $rc
