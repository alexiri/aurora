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

compose-up:
    bash deploy/compose/setup.sh
    {{compose}} up -d

compose-down:
    {{compose}} down
