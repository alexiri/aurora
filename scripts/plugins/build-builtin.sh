#!/usr/bin/env bash
# Build one `<id>-<version>.aurora-plugin` archive per package under plugins/builtin/, and sign
# them with the project key when one is supplied (docs/ops/builtin-plugin-signing.md).
#
#   bash scripts/plugins/build-builtin.sh --out <dir> [--aurora <binary>] [--key-file <file>]
#                                         [--build-only | --sign-only]
#
# A package with hooks keeps its guest crate in `plugins/builtin/<id>/hooks/`; `aurora plugin build`
# compiles it for wasm32-wasip2 (rustup target, `cargo build --locked`) and ships it as hooks.wasm.
#
# --key-file is the base64 seed written by `aurora plugin keygen` (the GitHub secret
# AURORA_PROJECT_SIGNING_KEY has exactly that content). When the file is missing or empty the
# archives are built UNSIGNED with a clear log line: the API's builtin directory refuses those
# (fail closed), they are only useful for dev (`plugins.dir` + `plugins.allow_unsigned`).
# The key file is read, never copied, and never printed.
#
# Compiling a hook guest runs third-party build scripts and proc-macros, so the image build splits
# the work: `--build-only` (compiles, no key anywhere near it) and then `--sign-only` (signs the
# archives that are already in --out; runs no cargo). Without either flag both happen in one go
# (local use).
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

OUT="" AURORA="$ROOT/target/debug/aurora" KEY="" BUILD=1 SIGN=1
while [ "$#" -gt 0 ]; do
  case "$1" in
    --out) OUT="$2"; shift 2 ;;
    --aurora) AURORA="$2"; shift 2 ;;
    --key-file) KEY="$2"; shift 2 ;;
    --build-only) SIGN=""; shift ;;
    --sign-only) BUILD=""; shift ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done
[ -n "$OUT" ] || { echo "usage: build-builtin.sh --out <dir> [--aurora <bin>] [--key-file <file>] [--build-only|--sign-only]" >&2; exit 2; }
[ -n "$BUILD" ] || [ -n "$SIGN" ] || { echo "--build-only and --sign-only are exclusive" >&2; exit 2; }
mkdir -p "$OUT"

signing=""
if [ -n "$SIGN" ]; then
  if [ -n "$KEY" ] && [ -s "$KEY" ]; then
    signing=1
    echo "signing: project key present, archives will be signed"
    pubfile="$(mktemp)"
    trap 'rm -f "$pubfile"' EXIT
    # PUBKEY_OVERRIDE is for local tests with a throwaway key only.
    pub="${PUBKEY_OVERRIDE:-$(sed -n 's/^pub const AURORA_PROJECT_PUBLIC_KEY: &str = "\(.*\)";/\1/p' \
      services/aurora-api/src/domains/plugins/keys.rs)}"
    [ -n "$pub" ] || { echo "cannot read AURORA_PROJECT_PUBLIC_KEY from keys.rs" >&2; exit 1; }
    printf '%s\n' "$pub" > "$pubfile"
  else
    echo "signing: SKIPPED, no project signing key supplied; archives are UNSIGNED and the API's builtin directory will refuse them"
  fi
fi

n=0
for dir in plugins/builtin/*/; do
  dir="${dir%/}"
  id="$(basename "$dir")"
  version="$(sed -n 's/^  version: *\([0-9][0-9A-Za-z.+-]*\).*/\1/p' "$dir/plugin.yaml" | head -1)"
  [ -n "$version" ] || { echo "no package.version in $dir/plugin.yaml" >&2; exit 1; }
  archive="$OUT/$id-$version.aurora-plugin"
  if [ -n "$BUILD" ]; then
    "$AURORA" plugin build "$dir" --out "$archive" >/dev/null
    # hooks.wasm is in the archive exactly when the manifest declares hooks
    has_wasm=""; tar -tzf "$archive" | grep -qx hooks.wasm && has_wasm=1
    declares=""; grep -Eq '^  hooks: *\[.+\]' "$dir/plugin.yaml" && declares=1
    [ "$has_wasm" = "$declares" ] \
      || { echo "FATAL: $archive: hooks.wasm presence does not match permissions.hooks" >&2; exit 1; }
    [ -z "$has_wasm" ] || echo "hooks    $archive (hooks.wasm compiled from $dir/hooks)"
  else
    [ -f "$archive" ] || { echo "FATAL: $archive does not exist: run --build-only first" >&2; exit 1; }
  fi
  if [ -n "$signing" ]; then
    "$AURORA" plugin sign "$archive" --key "$KEY" >/dev/null
    # The secret must be the private half of the compiled-in project key: verify against the
    # public key in keys.rs, so a wrong or stale secret fails the build instead of shipping
    # archives the API would refuse.
    "$AURORA" plugin validate "$archive" --pubkey "$pubfile" >/dev/null \
      || { echo "FATAL: $archive does not verify against AURORA_PROJECT_PUBLIC_KEY (services/aurora-api/src/domains/plugins/keys.rs): wrong signing secret?" >&2; exit 1; }
    echo "signed   $archive (verified against the compiled-in project public key)"
  elif [ -n "$BUILD" ]; then
    echo "unsigned $archive"
  fi
  n=$((n + 1))
done
echo "processed $n built-in package(s) in $OUT"
