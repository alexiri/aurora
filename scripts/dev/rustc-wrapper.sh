#!/usr/bin/env bash
# Cargo `build.rustc-wrapper` (.cargo/config.toml): route rustc through sccache when it is installed,
# so every worktree lane shares one compilation cache for third-party crates (OPS-016 measured a
# cold build at ~2x faster with a warm cache). Without sccache this is a plain pass-through, so CI,
# container builds and other machines are unaffected.
#
# Cargo calls: <wrapper> <path-to-rustc> <rustc args...>
# Overrides: AURORA_SCCACHE=0 disables; SCCACHE_PATH picks the binary (also found on PATH or in
# ~/.cargo/bin); SCCACHE_DIR / SCCACHE_CACHE_SIZE as usual.
set -euo pipefail
if [[ "${AURORA_SCCACHE:-1}" != "0" ]]; then
  for c in "${SCCACHE_PATH:-}" "$(command -v sccache 2>/dev/null || true)" "$HOME/.cargo/bin/sccache"; do
    if [[ -n "$c" && -x "$c" ]]; then
      export SCCACHE_CACHE_SIZE="${SCCACHE_CACHE_SIZE:-30G}"
      exec "$c" "$@"
    fi
  done
fi
exec "$@"
