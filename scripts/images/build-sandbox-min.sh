#!/usr/bin/env bash
# Builds the slim e2e sandbox image (OS + aurora-supervisor). Needs ~3 GB free for the Rust builder.
#   scripts/images/build-sandbox-min.sh [tag]    (default tag: aurora-sandbox-min:dev)
set -euo pipefail
cd "$(dirname "$0")/../.."
tag="${1:-aurora-sandbox-min:dev}"
free_kb=$(df --output=avail -k . | tail -1)
if [ "$free_kb" -lt $((15 * 1024 * 1024)) ]; then
  echo "refusing to build: less than 15 GB free" >&2
  exit 1
fi
base_args=()
[ -n "${SANDBOX_BASE_IMAGE:-}" ] && base_args=(--build-arg "SANDBOX_BASE_IMAGE=$SANDBOX_BASE_IMAGE")
podman build -f images/sandbox/Containerfile --target sandbox-min "${base_args[@]}" -t "$tag" .
podman image ls "$tag" --format '{{.Repository}}:{{.Tag}} {{.Size}}'
