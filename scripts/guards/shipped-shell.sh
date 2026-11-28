#!/usr/bin/env bash
# FIX-287: every shell script the apps and templates ship is checked; agents run them in their sandbox.
# A shell script is a file ending in .sh, or one whose first line is a shell shebang (app skill scripts
# have no extension).
#
#   shipped-shell.sh              check plugins/ and templates/
#   shipped-shell.sh --self-test  prove broken scripts are found and fail, and that a missing or
#                                 script-less directory fails (FIX-068: no vacuous pass)
#
# Runs shellcheck. Without it, falls back to a syntax check (bash -n, or sh -n for a sh shebang);
# never with CI=true.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

SHEBANG='^#![[:space:]]*([^[:space:]]*/)?(env[[:space:]]+(-S[[:space:]]+)?)?(sh|bash|dash|ksh)([[:space:]]|$)'
SH_SHEBANG='^#![[:space:]]*([^[:space:]]*/)?(env[[:space:]]+(-S[[:space:]]+)?)?(sh|dash)([[:space:]]|$)'

# check <dir>...: exit 0 = all scripts pass, 1 = a finding, 2 = a directory is missing or holds none.
check() {
  local d f first s rc=0 found
  local scripts=()
  for d in "$@"; do
    [ -d "$d" ] || { echo "shipped-shell: FAIL: scan directory $d is missing" >&2; return 2; }
    found=0
    while IFS= read -r -d '' f; do
      first=""
      IFS= read -r first 2>/dev/null < "$f" || true
      if [[ "$f" == *.sh || "$first" =~ $SHEBANG ]]; then
        scripts+=("$f")
        found=1
      fi
    done < <(find "$d" \( -name target -o -name node_modules \) -prune -o -type f -print0)
    [ "$found" = 1 ] || { echo "shipped-shell: FAIL: no shell script under $d" >&2; return 2; }
  done

  if command -v shellcheck >/dev/null 2>&1; then
    shellcheck "${scripts[@]}" || rc=1
  elif [ "${CI:-}" = "true" ]; then
    echo "shipped-shell: FAIL: shellcheck is not installed" >&2
    return 2
  else
    for s in "${scripts[@]}"; do
      first=""
      IFS= read -r first 2>/dev/null < "$s" || true
      if [[ "$first" =~ $SH_SHEBANG ]]; then sh -n "$s" || rc=1; else bash -n "$s" || rc=1; fi
    done
  fi
  echo "shipped-shell: checked ${#scripts[@]} scripts"
  return "$rc"
}

if [ "${1:-}" = "--self-test" ]; then
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' EXIT
  die() { echo "shipped-shell: self-test FAIL: $1" >&2; exit 1; }
  # expect <exit code> <what> <dir>...
  expect() {
    local want="$1" what="$2" got=0
    shift 2
    check "$@" >"$tmp/out" 2>&1 || got=$?
    [ "$got" = "$want" ] || { cat "$tmp/out" >&2; die "$what: exit $got, want $want"; }
  }
  mkdir -p "$tmp/clean/target" "$tmp/ext/deep" "$tmp/shebang" "$tmp/none"
  printf '#!/bin/sh\necho ok\n' > "$tmp/clean/a.sh"
  printf '#!/usr/bin/env python3\nif (:\n' > "$tmp/clean/tool"
  printf 'if true; then\n' > "$tmp/clean/target/build.sh"
  printf 'if true; then\n' > "$tmp/ext/deep/no-shebang.sh"
  printf '#!/usr/bin/env bash\nif true; then\n' > "$tmp/shebang/no-extension"
  printf '#!/usr/bin/env python3\n' > "$tmp/none/tool"
  expect 0 "clean scripts rejected (python and build output are not checked)" "$tmp/clean"
  grep -qF 'checked 1 scripts' "$tmp/out" || die "clean directory: want exactly 1 script"
  expect 1 "broken .sh file passed" "$tmp/clean" "$tmp/ext"
  expect 1 "broken extensionless script passed" "$tmp/clean" "$tmp/shebang"
  expect 2 "directory without scripts passed" "$tmp/clean" "$tmp/none"
  expect 2 "missing directory passed" "$tmp/clean" "$tmp/missing"
  echo "shipped-shell: self-test ok"
  exit 0
fi

check plugins templates || { echo "shipped-shell: FAIL: a shell script the apps or templates ship does not pass" >&2; exit 1; }
