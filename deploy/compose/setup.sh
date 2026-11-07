#!/usr/bin/env bash
# Idempotent compose setup: generates missing secrets in .env (never overwrites),
# creates the aurora-sandbox network, prints the first-admin command.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"

[ -f .env ] || cp .env.example .env
chmod 600 .env

# set_if_empty KEY VALUE: append or fill only when the key is absent/empty.
set_if_empty() {
  local key=$1 value=$2
  if grep -qE "^${key}=.+" .env; then return 0; fi
  if grep -qE "^${key}=" .env; then
    local tmp; tmp=$(mktemp)
    awk -v k="$key" -v v="$value" 'index($0, k"=")==1 {print k"="v; next} {print}' .env >"$tmp"
    cat "$tmp" >.env; rm -f "$tmp"
  else
    printf '%s=%s\n' "$key" "$value" >>.env
  fi
}

b64() { openssl rand -base64 32; }
set_if_empty POSTGRES_PASSWORD "$(openssl rand -hex 24)"
set_if_empty AURORA_ENCRYPTION_KEYS "k1:$(b64)"
set_if_empty AURORA_ENCRYPTION_ACTIVE_KEY k1
set_if_empty AURORA_SIGNING_KEY "$(b64)"
set_if_empty AURORA_INTERNAL_SECRET "$(b64)"
set_if_empty AURORA_RUNNER_SESSION_KEY "$(b64)"
set_if_empty AURORA_RUNNER_TOKEN "aur_$(openssl rand -hex 24)"
set_if_empty AURORA_CONNECTOR_TOKEN "aun_$(openssl rand -hex 24)"

set_if_empty AURORA_UID "$(id -u)"
set_if_empty AURORA_GID "$(id -g)"

# The hosted-profile runner talks to the rootless podman API socket (compose.yaml, service runner).
if ! systemctl --user is-active --quiet podman.socket 2>/dev/null; then
  echo "WARNING: podman.socket is not active; the hosted runner cannot start sandboxes." >&2
  echo "         Fix: systemctl --user enable --now podman.socket" >&2
fi

# --gateway is required so compose can assign static IPs (10.89.250.2/.3).
if ! podman network exists aurora-sandbox 2>/dev/null; then
  podman network create --internal --disable-dns --subnet 10.89.250.0/24 --gateway 10.89.250.1 aurora-sandbox
else
  echo "network aurora-sandbox exists"
fi

# Gateway CA (docs/15 §Development): minted once into the aurora-ca volume, which the gateway
# mounts at /var/lib/aurora/ca. generate-ca refuses to overwrite, so an existing CA is never
# touched: this only mints when the volume holds no CA. A new volume is root-owned and the
# images run as the distroless nonroot uid, so the volume root is chowned before minting.
CA_VOLUME=${AURORA_CA_VOLUME:-aurora-ca}   # override only for scratch tests
GATEWAY_IMAGE=aurora-gateway:${AURORA_VERSION:-dev}
GATEWAY_UID=65532
PROBE_IMAGE=docker.io/library/postgres:16  # has sh; the Aurora images are distroless

if podman run --rm -v "$CA_VOLUME:/ca:ro" --entrypoint /bin/sh "$PROBE_IMAGE" \
  -c 'test -f /ca/ca.pem && test -f /ca/ca.key'; then
  echo "gateway CA present in volume $CA_VOLUME; not regenerating"
else
  podman image exists "$GATEWAY_IMAGE" || podman compose -f compose.yaml build gateway
  podman run --rm --user 0:0 -v "$CA_VOLUME:/ca" --entrypoint /bin/sh "$PROBE_IMAGE" \
    -c "mkdir -p /ca && chown ${GATEWAY_UID}:${GATEWAY_UID} /ca"
  # Fails (set -e) if only one of ca.pem/ca.key exists: generate-ca refuses, and nothing is overwritten.
  podman run --rm -v "$CA_VOLUME:/ca" "$GATEWAY_IMAGE" generate-ca --out /ca
  echo "gateway CA written to volume $CA_VOLUME"
fi

# The API reads only the public ca.pem (gateway.ca_dir); export it from the volume to ./ca/ca.pem,
# which compose bind-mounts into the api service. ca.key never leaves the volume.
mkdir -p ca
podman run --rm -v "$CA_VOLUME:/ca:ro" --entrypoint /bin/sh "$PROBE_IMAGE" -c 'cat /ca/ca.pem' >ca/ca.pem.tmp
mv ca/ca.pem.tmp ca/ca.pem
chmod 644 ca/ca.pem; chmod 755 ca

cat <<MSG

.env ready at $(pwd)/.env
Start:  podman compose -f $(pwd)/compose.yaml up -d
Check:  curl -i localhost:4000/v1/instance ; curl -ik https://localhost:4102
Note:   distroless images have no curl, so compose only waits for service_started;
        verify health from the host through the published edge (4000) and gateway (4102) ports.
First admin:
  podman compose -f $(pwd)/compose.yaml run --rm api bootstrap-admin --email you@example.org
MSG
