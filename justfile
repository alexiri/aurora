set shell := ["bash", "-euo", "pipefail", "-c"]

# Start (or create) the dev Postgres on 127.0.0.1:55432.
dev-db:
    podman start aurora-dev-pg 2>/dev/null || podman run -d --name aurora-dev-pg \
      -e POSTGRES_USER=aurora -e POSTGRES_PASSWORD=aurora -e POSTGRES_DB=aurora \
      -p 127.0.0.1:55432:5432 docker.io/library/postgres:16 -c max_connections=300

# Cargo profile for the developer loop (I-36): incremental on for workspace members. Set AURORA_DEV_PROFILE=dev for the plain dev profile.
profile := env("AURORA_DEV_PROFILE", "dev-fast")

# Apply migrations to the dev DB.
migrate: dev-db
    . deploy/dev/env.sh && cargo run -q --profile {{profile}} -p aurora-api -- migrate

# Live-reload dev loop on the host (OPS-018, docs/ops/live-reload.md): Postgres, migrate, then API + gateway + hookhost + runner (fake backend; AURORA__RUNNER__BACKEND=podman for real sandboxes) rebuilt and restarted on save, plus `next dev` (Fast Refresh) on :4100. Needs python3 and `pnpm install` in web/ once.
dev: migrate
    #!/usr/bin/env bash
    set -euo pipefail
    command -v python3 >/dev/null || { echo "python3 not found: install python 3.8+ (the live-reload loop uses only its standard library)." >&2; exit 1; }
    [ -d web/node_modules ] || { echo "web/node_modules missing: run 'cd web && pnpm install --frozen-lockfile'." >&2; exit 1; }
    . deploy/dev/env.sh
    [ -f deploy/dev/ca/ca.pem ] || cargo run -q --profile {{profile}} -p aurora-gateway -- generate-ca --out deploy/dev/ca
    AURORA_DEV_PROFILE={{profile}} exec python3 scripts/dev/live.py

# --- containerized dev stack: kind + Helm (OPS-017; docs/operator/install-kind-dev.md) ---

# The Files journey on a kind cluster with the terminal on (P5-FILES-4, C-907): `AURORA_E2E_CLUSTER=aurora-e2e-<lane>` keeps it off the dev cluster. kind.sh flags pass through (--skip-build, --keep, --fresh).
e2e-files *flags:
    bash scripts/e2e/files.sh {{flags}}

# Create the kind cluster `aurora-dev`, build + load the images, helm install. Flags: --llm-mock --connector-fake --no-sandbox --tracing (dev Jaeger: `kubectl -n aurora port-forward svc/jaeger 16686`). One URL: http://localhost:4000.
kind-up *flags:
    bash deploy/dev/kind-dev.sh up {{flags}}

# Rebuild only the changed service images (api gateway hookhost runner connector-fake web sandbox; default: the core five), load them and restart their Deployments.
kind-dev *services:
    bash deploy/dev/kind-dev.sh dev {{services}}

# Watch the source and run `kind-dev` for the affected services on every save (OPS-018; ~35 s per change). Web: use `just dev` for Fast Refresh.
kind-live:
    #!/usr/bin/env bash
    set -euo pipefail
    command -v python3 >/dev/null || { echo "python3 not found: install python 3.8+." >&2; exit 1; }
    for t in kind kubectl helm podman; do command -v "$t" >/dev/null || { echo "kind-live needs '$t' (see docs/operator/install-kind-dev.md)." >&2; exit 1; }; done
    kind get clusters 2>/dev/null | grep -qx "${AURORA_DEV_CLUSTER:-aurora-dev}" || { echo "cluster not found: run 'just kind-up' first." >&2; exit 1; }
    exec python3 scripts/dev/live.py --kind

# Pods of the dev cluster.
kind-status:
    bash deploy/dev/kind-dev.sh status

# Delete the dev cluster (the cargo cache volume is kept).
kind-down:
    bash deploy/dev/kind-dev.sh down

# --- build, test and disk hygiene (OPS-016; docs/ops/build-performance.md) ---

# Whole workspace except the gated e2e crates.
test_excludes := "--exclude aurora-e2e-gateway --exclude aurora-e2e-hosted"

# Every crate but the e2e ones, one at a time through scripts/dev/test.sh, then the doctests. Extra args are nextest filters/args, e.g. `just test tenancy::`.
test *args:
    bash scripts/dev/test.sh "just-$(basename "$PWD")" rust --all-crates {{args}}
    # nextest does not run doctests (they need no database)
    bash scripts/dev/heavy.sh "just-$(basename "$PWD")" cargo test --workspace {{test_excludes}} --doc

# The same crate runs without the doctests (needs cargo-nextest, as `test` does).
test-nextest *args:
    bash scripts/dev/test.sh "just-$(basename "$PWD")" rust --all-crates {{args}}

# Incremental developer build of the services.
build-fast:
    cargo build --profile {{profile}} -p aurora-api -p aurora-gateway -p aurora-hookhost -p aurora-runner

# Fast local pre-commit gate (VEL-002): fmt + clippy + tests for what changed, web checks when web/ changed. NOT a merge gate — CI runs the full suite. Extra args pass through, e.g. `just quick -- --all`.
quick *args:
    bash scripts/dev/quick-gate.sh {{args}}

# Print target/ sizes, podman images/volumes by prefix, kind clusters and free disk.
disk-report *dirs:
    bash scripts/dev/disk-report.sh {{dirs}}

# Remove a finished lane's processes, podman objects and target/ (the coordinator runs this on merge).
clean-lane worktree *flags:
    bash scripts/dev/clean-lane.sh {{flags}} {{worktree}}

# Force-remove leftover aurora-testkit-* Postgres containers. The test kit no longer starts
# any (FIX-255: it uses the shared server of scripts/dev/test-pg.sh); this is for leftovers of
# runs from older trees. Only those containers carry the name prefix, never the shared dev DB or
# aurora-test-pg — but a checkout without FIX-255 may still be using one, so run this when no
# such test run is in progress.
clean-testkit:
    podman ps -a -q --filter name='^aurora-testkit-' | xargs -r podman rm -f -t 1
    # (FIX-103) Leaked per-test databases of killed runs live in the shared server: the testkit itself
    # reaps `aurora_test_<uuid>` databases with no connection older than 2 h (aurora_testkit::template::
    # reap_leaked) once per test process. To reap now, run any testkit test, e.g.
    #   scripts/dev/test.sh <lane> rust -p aurora-testkit cleanup_drops_database

# Drop build artefacts not used for DAYS days (cargo-sweep), else document-and-stop. Never touches other worktrees.
clean-stale days="14":
    #!/usr/bin/env bash
    set -euo pipefail
    # Incremental session dirs (profile.dev incremental, I-45): prune those untouched for DAYS days.
    # Safe while no build runs; a pruned crate just recompiles in full once.
    if [ -d target ]; then
      find target -maxdepth 3 -type d -name incremental -path 'target/*/incremental' 2>/dev/null | while read -r d; do
        find "$d" -mindepth 1 -maxdepth 1 -type d -mtime +{{days}} -print -exec rm -rf {} + || true
      done
    fi
    if cargo sweep --version >/dev/null 2>&1; then
      cargo sweep --time {{days}}
    else
      echo "cargo-sweep not installed (cargo install cargo-sweep --locked); only incremental dirs were pruned." >&2
      echo "Without it: 'cargo clean' (all of target/) when no build is running, or 'bash scripts/dev/clean-lane.sh <worktree>' for a finished lane." >&2
    fi
