#!/usr/bin/env bash
# Cargo `build.rustc-wrapper` (.cargo/config.toml): route rustc through sccache when it is installed,
# so every worktree lane shares one compilation cache for third-party crates. Without sccache this is
# a plain pass-through, so CI, container builds and other machines are unaffected.
#
# Cargo calls: <wrapper> <path-to-rustc> <rustc args...>
# Overrides: AURORA_SCCACHE=0 disables; SCCACHE_PATH picks the binary (also found on PATH or in
# ~/.cargo/bin); SCCACHE_DIR / SCCACHE_CACHE_SIZE as usual.
#
# Cache keys must not contain per-worktree paths, or no lane ever hits another lane's entries:
# - CARGO_TARGET_DIR: sccache hashes every CARGO_* env var; rustc never reads this one.
# - `-C linker=<worktree>/scripts/dev/linker.sh` (.cargo/config.toml): an absolute path that differs
#   per worktree. It is dropped only for pure lib/rlib crates that are not test harnesses: those never
#   invoke the linker (and are the only crates sccache caches), so the output is identical. Binaries,
#   tests, proc-macros and dylibs keep it and still link with lld.
set -euo pipefail

sccache_bin=""
if [[ "${AURORA_SCCACHE:-1}" != "0" ]]; then
  for c in "${SCCACHE_PATH:-}" "$(command -v sccache 2>/dev/null || true)" "$HOME/.cargo/bin/sccache"; do
    if [[ -n "$c" && -x "$c" ]]; then sccache_bin="$c"; break; fi
  done
fi
[[ -n "$sccache_bin" ]] || exec "$@"

export SCCACHE_CACHE_SIZE="${SCCACHE_CACHE_SIZE:-30G}"
unset CARGO_TARGET_DIR

args=("$@")
links=0
types=0
for ((i = 1; i < ${#args[@]}; i++)); do
  case "${args[i]}" in
    --test) links=1 ;;
    --crate-type) types=1; [[ "${args[i+1]:-}" =~ ^(lib|rlib)$ ]] || links=1 ;;
    --crate-type=*) types=1; [[ "${args[i]#--crate-type=}" =~ ^(lib|rlib)$ ]] || links=1 ;;
  esac
done
if ((types == 1 && links == 0)); then
  out=("${args[0]}")
  for ((i = 1; i < ${#args[@]}; i++)); do
    if [[ "${args[i]}" == -C && "${args[i+1]:-}" == linker=* ]]; then
      ((i += 1))
      continue
    fi
    [[ "${args[i]}" == -Clinker=* ]] && continue
    out+=("${args[i]}")
  done
  args=("${out[@]}")
fi
exec "$sccache_bin" "${args[@]}"
