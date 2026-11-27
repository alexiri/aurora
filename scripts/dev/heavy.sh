#!/usr/bin/env bash
# At most TWO CPU-heavy jobs at a time across every lane on this box (owner, 2026-11-19).
#
#   scripts/dev/heavy.sh <lane> <command> [args...]
#
# Waits for one of two machine-wide slots, records who holds it, runs the command at nice 10
# (so the desktop stays usable), and frees the slot when the command ends (also on failure or kill).
# Use it for every cargo build/check/test/clippy/nextest/deny, pnpm build/vitest/playwright,
# scripts/e2e/*.sh, scripts/fixtures/build.sh, check-builtin.sh, podman build. Not for edits,
# reads, git, grep, or a single small vitest file. Test runs go through scripts/dev/test.sh,
# which takes its own slot: do not wrap it in this script.
#
# The command runs with the slot descriptors closed (`8>&- 9>&-`), so a daemon it leaves behind
# (a test server, a preview server) can never keep holding a slot.
# Slot 0 is .heavy.lock (the single lock of the first version), slot 1 is .heavy.lock1.
#
# The lock directory is shared by every worktree (and by the old path outside the repo, which is a
# wrapper around this script): AURORA_HEAVY_DIR, default below. The command sees
# AURORA_HEAVY_HELD=<lock directory>, so a script that takes its own slot (test.sh) can refuse to
# run inside one instead of waiting for a second slot while holding the first.
set -u
DIR=${AURORA_HEAVY_DIR:-/home/alex/orca/workspaces/aurora}
WAIT=$DIR/.heavy.wait            # one file per waiter: `ls -l .heavy.wait` is the queue
lane="${1:?usage: heavy.sh <lane> <command> [args...]}"
shift
[ "$#" -gt 0 ] || { echo "usage: heavy.sh <lane> <command> [args...]" >&2; exit 2; }
mkdir -p "$WAIT"
me="$lane pid=$$ queued $(date '+%H:%M:%S') $(printf '%q ' "$@" | cut -c1-160)"
waitfile="$WAIT/$(date '+%s')-$$"
echo "$me" > "$waitfile"
trap 'rm -f "$waitfile"' EXIT
slot=""
while [ -z "$slot" ]; do
  exec 9>"$DIR/.heavy.lock"
  if flock -n 9; then slot=0; break; fi
  exec 9>&-
  exec 9>"$DIR/.heavy.lock1"
  if flock -n 9; then slot=1; break; fi
  exec 9>&-
  sleep $((2 + RANDOM % 4))
done
STATUS=$DIR/.heavy.status; [ "$slot" = 1 ] && STATUS=$DIR/.heavy.status1
rm -f "$waitfile"
echo "HOLDER slot $slot since $(date '+%H:%M:%S'): $me" > "$STATUS"
unset CARGO_BUILD_JOBS
AURORA_HEAVY_HELD=$DIR nice -n 10 "$@" 8>&- 9>&-
rc=$?
echo "idle since $(date '+%H:%M:%S') (last: $lane rc=$rc)" > "$STATUS"
exit $rc
