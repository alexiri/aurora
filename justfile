set shell := ["bash", "-euo", "pipefail", "-c"]

# Start (or create) the dev Postgres on 127.0.0.1:55432.
dev-db:
    podman start aurora-dev-pg 2>/dev/null || podman run -d --name aurora-dev-pg \
      -e POSTGRES_USER=aurora -e POSTGRES_PASSWORD=aurora -e POSTGRES_DB=aurora \
      -p 127.0.0.1:55432:5432 docker.io/library/postgres:16

# Cargo profile for the developer loop (I-36): incremental on for workspace members. Set AURORA_DEV_PROFILE=dev for the plain dev profile.
profile := env("AURORA_DEV_PROFILE", "dev-fast")

# Apply migrations to the dev DB.
migrate: dev-db
    . deploy/dev/env.sh && cargo run -q --profile {{profile}} -p aurora-api -- migrate

# Dev loop on the host: Postgres, migrate, API + gateway + hookhost + web (runner not started; needs pnpm install in web/ once).
dev: migrate
    #!/usr/bin/env bash
    set -euo pipefail
    . deploy/dev/env.sh
    [ -f deploy/dev/ca/ca.pem ] || cargo run -q --profile {{profile}} -p aurora-gateway -- generate-ca --out deploy/dev/ca
    trap 'kill 0' EXIT
    cargo run -q --profile {{profile}} -p aurora-api -- serve &
    cargo run -q --profile {{profile}} -p aurora-gateway -- serve &
    cargo run -q --profile {{profile}} -p aurora-hookhost -- serve &
    pnpm --dir web dev &
    wait

# --- containerized dev stack: kind + Helm (OPS-017; docs/operator/install-kind-dev.md) ---

# Create the kind cluster `aurora-dev`, build + load the images, helm install. Flags: --llm-mock --connector-fake --no-sandbox. One URL: http://localhost:4000.
kind-up *flags:
    bash deploy/dev/kind-dev.sh up {{flags}}

# Rebuild only the changed service images (api gateway hookhost runner connector-fake web sandbox; default: the core five), load them and restart their Deployments.
kind-dev *services:
    bash deploy/dev/kind-dev.sh dev {{services}}

# Pods of the dev cluster.
kind-status:
    bash deploy/dev/kind-dev.sh status

# Delete the dev cluster (the cargo cache volume is kept).
kind-down:
    bash deploy/dev/kind-dev.sh down

# --- build, test and disk hygiene (OPS-016; docs/ops/build-performance.md) ---

# Whole workspace except the gated e2e crates.
test_excludes := "--exclude aurora-e2e-gateway --exclude aurora-e2e-hosted"

# Workspace tests (no e2e) with `cargo test` (what CI runs). Extra args pass through, e.g. `just test -- tenancy::`.
test *args:
    cargo test --workspace {{test_excludes}} {{args}}

# Same set under cargo-nextest (opt-in: `cargo install cargo-nextest --locked`; not a measured win, see docs/ops/build-performance.md).
test-nextest *args:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo nextest --version >/dev/null 2>&1 || { echo "cargo-nextest not installed: cargo install cargo-nextest --locked" >&2; exit 1; }
    cargo nextest run --workspace {{test_excludes}} {{args}}
    # nextest does not run doctests
    cargo test --workspace {{test_excludes}} --doc

# Incremental developer build of the services.
build-fast:
    cargo build --profile {{profile}} -p aurora-api -p aurora-gateway -p aurora-hookhost -p aurora-runner

# Print target/ sizes, podman images/volumes by prefix, kind clusters and free disk.
disk-report *dirs:
    bash scripts/dev/disk-report.sh {{dirs}}

# Remove a finished lane's processes, podman objects and target/ (the coordinator runs this on merge).
clean-lane worktree *flags:
    bash scripts/dev/clean-lane.sh {{flags}} {{worktree}}

# Drop build artefacts not used for DAYS days (cargo-sweep), else document-and-stop. Never touches other worktrees.
clean-stale days="14":
    #!/usr/bin/env bash
    set -euo pipefail
    if cargo sweep --version >/dev/null 2>&1; then
      cargo sweep --time {{days}}
    else
      echo "cargo-sweep not installed (cargo install cargo-sweep --locked)." >&2
      echo "Without it: 'cargo clean' (all of target/) when no build is running, or 'bash scripts/dev/clean-lane.sh <worktree>' for a finished lane." >&2
      exit 1
    fi
