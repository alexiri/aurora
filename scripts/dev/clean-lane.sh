#!/usr/bin/env bash
# OPS-016: leave nothing behind when a worker lane (git worktree) is finished.
#
#   scripts/dev/clean-lane.sh [--dry-run] [--prefix P]... [--keep-target] <worktree>
#
# 1. kills processes whose /proc/PID/cwd is inside <worktree> (never this script or its parents),
# 2. removes podman containers, pods, networks and volumes whose NAME starts with a lane prefix
#    (default prefixes: "<worktree-basename>" and "aurora-<worktree-basename>") or that carry the
#    label aurora.lane=<worktree-basename>,
# 3. removes <worktree>/target.
#
# The shared dev Postgres (aurora-dev-pg) is never touched. The worktree itself is removed by the
# coordinator (`git worktree remove`), after this script.
set -uo pipefail

DRY=0; KEEP_TARGET=0; PREFIXES=(); WT=""
while [ $# -gt 0 ]; do
  case "$1" in
    --dry-run) DRY=1 ;;
    --keep-target) KEEP_TARGET=1 ;;
    --prefix) shift; PREFIXES+=("${1:?--prefix needs a value}") ;;
    -h|--help) sed -n '2,12p' "$0"; exit 0 ;;
    -*) echo "unknown option $1" >&2; exit 2 ;;
    *) if [ -z "$WT" ]; then WT="$1"; else echo "only one worktree" >&2; exit 2; fi ;;
  esac
  shift
done
[ -n "$WT" ] || { echo "usage: $0 [--dry-run] [--prefix P]... <worktree>" >&2; exit 2; }
[ -d "$WT" ] || { echo "no such directory: $WT" >&2; exit 2; }
WT="$(cd "$WT" && pwd -P)"

# Only linked worktrees (.git is a file); never the primary checkout, / or $HOME.
[ "$WT" != "/" ] && [ "$WT" != "$HOME" ] || { echo "refusing to clean $WT" >&2; exit 2; }
[ -e "$WT/.git" ] || { echo "$WT has no .git (not a worktree); refusing" >&2; exit 2; }
if [ -d "$WT/.git" ]; then
  echo "$WT is a primary checkout (.git is a directory), not a linked worktree; refusing" >&2; exit 2
fi

BASE="$(basename "$WT")"
[ ${#PREFIXES[@]} -gt 0 ] || PREFIXES=("$BASE" "aurora-$BASE")
for p in "${PREFIXES[@]}"; do
  case "$p" in aurora|aurora-|test|e2e|dev) echo "prefix '$p' is too broad" >&2; exit 2 ;; esac
  [ "${#p}" -ge 4 ] || { echo "prefix '$p' is too short (min 4)" >&2; exit 2; }
done

run() { if [ "$DRY" = 1 ]; then echo "DRY: $*"; else "$@"; fi; }

echo "== lane $WT (prefixes: ${PREFIXES[*]})"

# 1. stray processes by cwd (skip this script and its ancestors)
protect=" $$ "; pp=$$
while [ "$pp" -gt 1 ]; do pp=$(awk '{print $4}' "/proc/$pp/stat" 2>/dev/null || echo 1); protect+="$pp "; done
in_wt() { local cwd; cwd="$(readlink "/proc/$1/cwd" 2>/dev/null)" || return 1; case "$cwd" in "$WT"|"$WT"/*) return 0 ;; esac; return 1; }
killed=0
for d in /proc/[0-9]*; do
  pid=${d#/proc/}
  case "$protect" in *" $pid "*) continue ;; esac
  in_wt "$pid" || continue
  echo "kill $pid ($(tr '\0' ' ' < "$d/cmdline" 2>/dev/null | cut -c1-100))"
  run kill "$pid" 2>/dev/null; killed=$((killed+1))
done
if [ "$DRY" = 0 ] && [ "$killed" -gt 0 ]; then
  sleep 2
  for d in /proc/[0-9]*; do
    pid=${d#/proc/}; case "$protect" in *" $pid "*) continue ;; esac
    in_wt "$pid" && { echo "kill -9 $pid"; kill -9 "$pid" 2>/dev/null; }
  done
fi
echo "processes signalled: $killed"

# 2. podman objects
if command -v podman >/dev/null 2>&1; then
  match() { # stdin: names; prints those starting with a lane prefix (never the shared dev DB)
    local n p
    while IFS= read -r n; do
      [ -z "$n" ] && continue
      [ "$n" = aurora-dev-pg ] && continue
      for p in "${PREFIXES[@]}"; do case "$n" in "$p"*) echo "$n"; break ;; esac; done
    done
  }
  { podman ps -a --format '{{.Names}}' | match; podman ps -a --format '{{.Names}}' --filter "label=aurora.lane=$BASE"; } | sort -u | while read -r c; do
    echo "container $c"; run podman rm -f -t 1 "$c" >/dev/null 2>&1
  done
  podman pod ls --format '{{.Name}}' 2>/dev/null | match | while read -r x; do echo "pod $x"; run podman pod rm -f "$x" >/dev/null 2>&1; done
  podman network ls --format '{{.Name}}' | match | while read -r x; do echo "network $x"; run podman network rm -f "$x" >/dev/null 2>&1; done
  { podman volume ls --format '{{.Name}}' | match; podman volume ls --format '{{.Name}}' --filter "label=aurora.lane=$BASE"; } | sort -u | while read -r x; do
    echo "volume $x"; run podman volume rm -f "$x" >/dev/null 2>&1
  done
else
  echo "podman not installed; skipping container cleanup"
fi

# 3. build artefacts
if [ "$KEEP_TARGET" = 0 ] && [ -d "$WT/target" ]; then
  echo "remove $WT/target ($(du -sh "$WT/target" 2>/dev/null | cut -f1))"
  run rm -rf "$WT/target"
fi
echo "== lane clean"
