#!/usr/bin/env bash
# M2 acceptance: real aurora-api + 2x aurora-gateway + Postgres + mock upstreams (TEST-012).
# The binaries hard-code ports 4101/4111 and 4102/4103; the suite isolates itself by binding
# them on 127.0.0.31/.32/.33. Tests run serially; each creates and drops its own database.
set -uo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"
for ip in 127.0.0.31 127.0.0.32 127.0.0.33; do
  for port in 4101 4102 4103 4111; do
    if ss -ltn "sport = :$port" | grep -q "$ip:"; then echo "$ip:$port already in use" >&2; exit 1; fi
  done
done
cargo build -p aurora-api -p aurora-gateway || { echo "cargo build failed" >&2; exit 1; }
cargo test -p aurora-e2e-gateway -- --test-threads=1 "$@"
