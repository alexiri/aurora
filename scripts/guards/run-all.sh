#!/usr/bin/env bash
# Runs every architecture guard and the schema drift check. Fails on the first failure.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

echo "== schema guard (docs/03 vs migrations)"
python3 scripts/extract-schema.py --check

echo "== no-integration-names (SEC-33)"
bash scripts/guards/no-integration-names.sh

echo "== dep-graph"
bash scripts/guards/dep-graph.sh

echo "== edge-routes (Helm Ingress/HTTPRoute)"
bash scripts/guards/edge-routes.sh

echo "== all guards passed"
