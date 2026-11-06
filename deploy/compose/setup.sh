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

# --gateway is required so compose can assign static IPs (10.89.250.2/.3).
if ! podman network exists aurora-sandbox 2>/dev/null; then
  podman network create --internal --disable-dns --subnet 10.89.250.0/24 --gateway 10.89.250.1 aurora-sandbox
else
  echo "network aurora-sandbox exists"
fi

echo "TODO(M2): gateway CA generation skipped (aurora-gateway generate-ca not implemented yet):"
echo "  podman run --rm -v aurora-ca:/ca aurora-gateway generate-ca --out /ca"

cat <<MSG

.env ready at $(pwd)/.env
Start:  podman compose -f $(pwd)/compose.yaml up -d
Check:  curl -i localhost:4000/v1/instance ; curl -i localhost:4102
Note:   distroless images have no curl, so compose only waits for service_started;
        verify health from the host through the published edge (4000) and gateway (4102) ports.
First admin (available once BE-016 lands):
  podman compose -f $(pwd)/compose.yaml run --rm api bootstrap-admin --email you@example.org
MSG
