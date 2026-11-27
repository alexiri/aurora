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
# First come, first served across both slots (FIX-255). Each job takes the next number on arrival
# (.heavy.ticket.seq, under the lock .heavy.ticket.lock) and has a file .heavy.wait/q-<number>
# while it waits; only the oldest waiting number may try for a free slot. A waiter holds a lock on
# its ticket file for as long as it lives, so a ticket that can be locked belongs to a dead
# process and is dropped. (Before this, a job arriving as a slot freed usually took it ahead of
# jobs that had waited for hours.) A caller of the old script has no ticket: it competes for the
# slot locks with the oldest ticket, and nobody ever holds a slot while waiting for a ticket.
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

# Take a ticket. The number, the locked ticket file and its appearance in the queue happen under
# one lock, so a ticket is never visible before an older one, nor before it is locked. Descriptor
# 6 holds the ticket's own lock until this process has a slot (or dies).
exec 5>"$DIR/.heavy.ticket.lock"
flock 5 || { echo "heavy.sh: cannot lock $DIR/.heavy.ticket.lock" >&2; exit 1; }
number=$(cat "$DIR/.heavy.ticket.seq" 2>/dev/null)
case "$number" in ''|*[!0-9]*) number=0 ;; esac
number=$((10#$number))
# Never below a ticket that is still queued, even if the counter file was lost.
for f in "$WAIT"/q-*; do
  n=${f##*/q-}
  case "$n" in ''|*[!0-9]*) continue ;; esac
  [ $((10#$n)) -le "$number" ] || number=$((10#$n))
done
number=$((number + 1))
echo "$number" > "$DIR/.heavy.ticket.seq"
waitfile="$WAIT/q-$(printf '%012d' "$number")"
exec 6>"$WAIT/.new-$$"
if ! { flock -n 6 && echo "$me" >&6 && mv "$WAIT/.new-$$" "$waitfile"; }; then
  echo "heavy.sh: cannot create a ticket in $WAIT" >&2
  rm -f "$WAIT/.new-$$"
  exit 1
fi
trap 'rm -f "$waitfile"' EXIT
exec 5>&-

# True when no older ticket of a living waiter is queued. Drops the tickets of dead ones.
oldest() {
  local f n
  for f in "$WAIT"/q-*; do
    n=${f##*/q-}
    case "$n" in ''|*[!0-9]*) continue ;; esac
    [ $((10#$n)) -lt "$number" ] || continue
    if (exec 5<"$f" && flock -n 5) 2>/dev/null; then
      rm -f "$f"
    elif [ -e "$f" ]; then
      return 1
    fi
  done
  return 0
}

slot=""
while [ -z "$slot" ]; do
  if oldest; then
    exec 9>"$DIR/.heavy.lock"
    if flock -n 9; then slot=0; break; fi
    exec 9>&-
    exec 9>"$DIR/.heavy.lock1"
    if flock -n 9; then slot=1; break; fi
    exec 9>&-
    sleep 1 6>&-
  else
    sleep $((2 + RANDOM % 4)) 6>&-
  fi
done
STATUS=$DIR/.heavy.status; [ "$slot" = 1 ] && STATUS=$DIR/.heavy.status1
rm -f "$waitfile"
exec 6>&-
echo "HOLDER slot $slot since $(date '+%H:%M:%S'): $me" > "$STATUS"
unset CARGO_BUILD_JOBS
AURORA_HEAVY_HELD=$DIR nice -n 10 "$@" 5>&- 6>&- 8>&- 9>&-
rc=$?
echo "idle since $(date '+%H:%M:%S') (last: $lane rc=$rc)" > "$STATUS"
exit $rc
