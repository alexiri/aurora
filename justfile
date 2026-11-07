set shell := ["bash", "-euo", "pipefail", "-c"]

compose := "podman compose -f deploy/compose/compose.yaml"

# Start (or create) the dev Postgres on 127.0.0.1:55432.
dev-db:
    podman start aurora-dev-pg 2>/dev/null || podman run -d --name aurora-dev-pg \
      -e POSTGRES_USER=aurora -e POSTGRES_PASSWORD=aurora -e POSTGRES_DB=aurora \
      -p 127.0.0.1:55432:5432 docker.io/library/postgres:16

# Apply migrations to the dev DB.
migrate: dev-db
    . deploy/dev/env.sh && cargo run -q -p aurora-api -- migrate

# Dev loop on the host: Postgres, migrate, API + gateway + hookhost + web (runner not started; needs pnpm install in web/ once).
dev: migrate
    #!/usr/bin/env bash
    set -euo pipefail
    . deploy/dev/env.sh
    [ -f deploy/dev/ca/ca.pem ] || cargo run -q -p aurora-gateway -- generate-ca --out deploy/dev/ca
    trap 'kill 0' EXIT
    cargo run -q -p aurora-api -- serve &
    cargo run -q -p aurora-gateway -- serve &
    cargo run -q -p aurora-hookhost -- serve &
    pnpm --dir web dev &
    wait

compose-up:
    bash deploy/compose/setup.sh
    {{compose}} up -d

compose-down:
    {{compose}} down
