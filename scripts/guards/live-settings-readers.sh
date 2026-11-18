#!/usr/bin/env bash
# FIX-168 (SET-5, SEC-125): the operational settings are read live (`AppState::ops()`), never from
# the boot snapshot. A reader of `<x>.config.<section>.<key>` for a section `install_live_ops` swaps
# (`app.rs`, the PREFIXES list) sees only the config file, not the merged setting: the Helm chart
# renders `terminal.available` only, so `config.terminal.enabled` stayed false (Files said "not
# available" while the terminal worked) and a live "off" never reached the reader.
#
# Keys that are configuration-only (`NOT_SETTINGS` in aurora-config) are listed in
# `scripts/guards/live-settings-readers.allow`, one `section.key reason` per line. The list is a
# ratchet: an entry nobody reads any more fails the guard until it is removed.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

APP=services/aurora-api/src/app.rs
ALLOW=scripts/guards/live-settings-readers.allow

# The sections install_live_ops swaps, from the source (so a new section is covered at once).
sections="$(sed -n '/const PREFIXES/,/\];/p' "$APP" | grep -oE '"[a-z_]+\.' | tr -d '".' | sort -u | paste -sd'|')"
if [ -z "$sections" ]; then
  echo "live-settings guard: cannot read the PREFIXES list of $APP" >&2
  exit 1
fi

found="$(grep -rnoE "\.config\.($sections)\.[a-z_]+" services crates --include='*.rs' \
  | grep -vE '/(tests|target)/' | grep -vE '/tests\.rs:' || true)"

fail=0
keys="$(printf '%s\n' "$found" | sed -E '/^$/d; s/^[^:]+:[0-9]+:\.config\.//' | sort -u)"
for k in $keys; do
  if ! grep -qE "^$k[[:space:]]" "$ALLOW" 2>/dev/null; then
    echo "live-settings guard: a reader of the boot config's '$k' (a live setting): use state.ops() (SET-5), or list it in $ALLOW if it is configuration-only (NOT_SETTINGS):" >&2
    printf '%s\n' "$found" | grep -E ":\.config\.$k\$" | sed 's/^/  /' >&2
    fail=1
  fi
done
while read -r k _; do
  case "$k" in ''|'#'*) continue ;; esac
  if ! printf '%s\n' "$keys" | grep -qx "$k"; then
    echo "live-settings guard: '$k' is listed in $ALLOW but nothing reads it any more; remove the line" >&2
    fail=1
  fi
done < "$ALLOW"
exit $fail
