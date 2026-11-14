#!/usr/bin/env bash
# Supply chain (FIX-103, docs/17): every base image in images/**/Containerfile* (and the e2e Containerfile)
# is pinned by digest. A `FROM <image>` must carry `@sha256:<64 hex>`; `FROM ${ARG}` is resolved to that
# ARG's default in the same file and the default must carry the digest; `FROM <earlier stage>` is a stage
# reference. The Helm chart's bundled Postgres image is checked the same way. A deliberate exception (an
# `ARG BASE=` that the caller always overrides) is the e2e overlay `scripts/e2e/llm.Containerfile`, which
# builds on a locally built image and is allowed by name below.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

fail() { echo "image-pins: FAIL: $*" >&2; exit 1; }

DIGEST='@sha256:[0-9a-f]{64}$'
# Files whose FROM is a locally built image by design.
ALLOW_LOCAL="scripts/e2e/llm.Containerfile"

# check_file <path>: prints violations, one per line.
check_file() {
  awk -v file="$1" -v dig="$DIGEST" '
    /^[[:space:]]*ARG[[:space:]]+[A-Za-z_][A-Za-z0-9_]*=/ {
      line = $0; sub(/^[[:space:]]*ARG[[:space:]]+/, "", line)
      name = line; sub(/=.*/, "", name); val = line; sub(/^[^=]*=/, "", val); gsub(/"/, "", val)
      args[name] = val
    }
    /^[[:space:]]*FROM[[:space:]]/ {
      n = split($0, w, /[[:space:]]+/)
      i = 2
      if (w[i] ~ /^--platform/) i++
      img = w[i]
      stage = ""
      for (j = i + 1; j <= n; j++) if (toupper(w[j]) == "AS") stage = w[j + 1]
      resolved = img
      if (img ~ /^\$\{[A-Za-z_][A-Za-z0-9_]*\}$/) {
        v = img; gsub(/[${}]/, "", v)
        if (!(v in args)) { printf "%s: FROM %s has no ARG default in this file\n", file, img; next }
        resolved = args[v]
      }
      if (resolved in stages) { if (stage != "") stages[stage] = 1; next }
      if (resolved == "scratch") { if (stage != "") stages[stage] = 1; next }
      if (resolved !~ dig) printf "%s: FROM %s (%s) is not pinned by @sha256:<digest>\n", file, img, resolved
      if (stage != "") stages[stage] = 1
    }
  ' "$1"
}

files="$(find images -type f \( -name 'Containerfile*' -o -name '*.Containerfile' \) | sort)"
[ -n "$files" ] || fail "no Containerfiles found under images/"
bad=""
for f in $files; do
  out="$(check_file "$f")"
  [ -z "$out" ] || bad="$bad$out"$'\n'
done
# The e2e overlay: allowed to start from a local image, nothing else.
for f in $ALLOW_LOCAL; do
  [ -f "$f" ] || continue
  grep -qE '^ARG BASE=localhost/' "$f" || bad="$bad$f: expected ARG BASE=localhost/... (a locally built image)"$'\n'
done
[ -z "$bad" ] || { printf '%s' "$bad" >&2; fail "unpinned base images (resolve the digest with 'skopeo inspect --raw docker://<image> | sha256sum' and write <image>@sha256:<digest>)"; }

# --- the chart's bundled Postgres ---
grep -Eq "^  image: docker\.io/library/postgres:[A-Za-z0-9._-]+${DIGEST}" deploy/helm/aurora/values.yaml \
  || fail "deploy/helm/aurora/values.yaml: postgres.image must be pinned by digest"

# --- self-test: a planted unpinned FROM, and a planted unpinned ARG default, must be reported ---
tmp="$(mktemp)"
trap 'rm -f "$tmp"' EXIT
printf 'ARG B=docker.io/library/rust:1\nFROM ${B} AS chef\nFROM docker.io/library/node:22\nFROM chef AS x\n' >"$tmp"
[ "$(check_file "$tmp" | wc -l)" -eq 2 ] || fail "self-test: the scan did not report exactly the two planted unpinned FROMs"
printf 'ARG B=docker.io/library/rust:1@sha256:%064d\nFROM ${B} AS chef\nFROM chef AS x\n' 0 >"$tmp"
[ -z "$(check_file "$tmp")" ] || fail "self-test: a pinned ARG default and a stage reference were reported"

echo "image-pins: ok ($(printf '%s\n' "$files" | wc -l) Containerfiles, every base image pinned by digest)"
