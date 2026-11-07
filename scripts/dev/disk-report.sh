#!/usr/bin/env bash
# OPS-016: where is the disk going? Read-only.
#   scripts/dev/disk-report.sh [dir ...]
# Prints free space, target/ sizes (every `git worktree list` entry plus any extra dirs given),
# sccache stats, podman images/containers/volumes grouped by name prefix, and kind clusters.
set -uo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

echo "== free space"
df -h "$ROOT" | sed -n '1,2p'

echo; echo "== target/ directories (largest first)"
{
  git worktree list --porcelain 2>/dev/null | awk '/^worktree /{print $2}'
  for r in "$@"; do find "$r" -maxdepth 3 -name target -type d -prune -printf '%h\n' 2>/dev/null; done
} | sort -u | while read -r w; do
  [ -d "$w/target" ] && printf '%8s  %s\n' "$(du -sh "$w/target" 2>/dev/null | cut -f1)" "$w/target"
done | sort -h -r
[ -n "${CARGO_TARGET_DIR:-}" ] && printf '%8s  %s (CARGO_TARGET_DIR)\n' "$(du -sh "$CARGO_TARGET_DIR" 2>/dev/null | cut -f1)" "$CARGO_TARGET_DIR"

echo; echo "== sccache"
if command -v sccache >/dev/null 2>&1; then sccache --show-stats 2>/dev/null | grep -Ei 'location|cache size|max cache|hits|misses'; else echo "not installed (optional)"; fi

echo; echo "== podman"
if command -v podman >/dev/null 2>&1; then
  podman system df 2>/dev/null
  echo "-- images"; podman images --format '{{.Repository}}:{{.Tag}} {{.Size}}' | sort | head -40
  echo "-- containers by prefix (all states)"
  podman ps -a --format '{{.Names}}' | sed -E 's/^([a-z0-9]+(-[a-z0-9]+)?).*/\1/' | sort | uniq -c | sort -rn
  echo "-- volumes by prefix"
  podman volume ls --format '{{.Name}}' | cut -c1-14 | sort | uniq -c | sort -rn | head -20
  echo "-- networks"; podman network ls --format '{{.Name}}' | tr '\n' ' '; echo
else echo "podman not installed"; fi

echo; echo "== kind clusters"
if command -v kind >/dev/null 2>&1; then kind get clusters 2>&1; else echo "kind not installed"; fi
