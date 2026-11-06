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
set -uo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"
if [ "${AURORA_TEST_PODMAN:-}" != "1" ]; then echo "skipped: set AURORA_TEST_PODMAN=1" >&2; exit 0; fi

SBX_NET=aurora-e2eh-sbx
DB_NET=aurora-e2eh-db
PG=aurora-e2eh-pg
ANCHOR=aurora-e2eh-anchor
TOKEN="aur_e2ehostedtoken"
FP=$(printf '%s' "$TOKEN" | sha256sum | cut -c1-16)
IMAGE="localhost/aurora-sandbox:0.1.0"

cleanup() {
  podman ps -a -q --filter "label=aurora.installation=$FP" | xargs -r podman rm -f -t 1 >/dev/null 2>&1
  podman volume ls -q --filter "label=aurora.installation=$FP" | xargs -r podman volume rm -f >/dev/null 2>&1
  podman rm -f -t 1 "$PG" "$ANCHOR" >/dev/null 2>&1
  podman network rm "$SBX_NET" "$DB_NET" >/dev/null 2>&1
}
trap cleanup EXIT
cleanup

free_kb=$(df --output=avail -k . | tail -1)
if [ "$free_kb" -lt $((15 * 1024 * 1024)) ]; then echo "less than 15 GB free; stopping" >&2; exit 1; fi

cargo build -p aurora-api -p aurora-gateway -p aurora-runner || { echo "cargo build failed" >&2; exit 1; }
if [ "${SKIP_IMAGE_BUILD:-}" != "1" ]; then
  scripts/images/build-sandbox-min.sh aurora-sandbox:0.1.0 || { echo "image build failed" >&2; exit 1; }
fi
podman image exists "$IMAGE" || { echo "missing $IMAGE" >&2; exit 1; }
cargo test -p aurora-e2e-hosted --no-run || exit 1

podman network create --internal --disable-dns --subnet 10.89.251.0/24 --gateway 10.89.251.1 "$SBX_NET" >/dev/null || exit 1
podman network create --disable-dns --subnet 10.89.252.0/24 --gateway 10.89.252.1 "$DB_NET" >/dev/null || exit 1
podman run -d --name "$ANCHOR" --network "$SBX_NET" --entrypoint sleep "$IMAGE" 100000 >/dev/null || exit 1
podman run -d --name "$PG" --network "$DB_NET" --ip 10.89.252.5 \
  -e POSTGRES_USER=aurora -e POSTGRES_PASSWORD=aurora -e POSTGRES_DB=postgres \
  docker.io/library/postgres:16 -c max_connections=300 -c fsync=off >/dev/null || exit 1
for _ in $(seq 60); do
  podman exec "$PG" pg_isready -U aurora >/dev/null 2>&1 && break
  sleep 1
done

SOCK=$(podman info --format '{{.Host.RemoteSocket.Path}}')
podman unshare --rootless-netns env \
  AURORA_TEST_DATABASE_URL="postgres://aurora:aurora@10.89.252.5:5432/postgres" \
  AURORA_E2EH_SOCK="$SOCK" AURORA_E2EH_NETWORK="$SBX_NET" AURORA_E2EH_TOKEN="$TOKEN" \
  AURORA_E2EH_FP="$FP" AURORA_E2EH_IMAGE="$IMAGE" \
  cargo test -p aurora-e2e-hosted -- --test-threads=1 --nocapture "$@"
rc=$?
exit $rc
