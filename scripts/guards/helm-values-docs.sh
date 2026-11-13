#!/usr/bin/env bash
# docs/23 <-> values.yaml drift guard (FIX-069; the owner asked for docs/23 to be the comprehensive
# values reference, so a row for a key that does not exist, or a real key with no row, is a defect).
#   1. every leaf key of deploy/helm/aurora/values.yaml is documented in docs/23-helm-values.md;
#   2. every values key docs/23 names (a backticked dotted path whose first segment is a top-level
#      values.yaml key) exists in values.yaml.
# No python and no yq (the python and Go "yq" are different tools with different syntax): values.yaml
# is walked by the small awk parser below. It understands only what this file uses: `key:` lines
# nested by indentation, `- ` list items (the list is a leaf), inline `{}`/`[]`/scalar values.
#
# How docs/23 names keys (the matcher accepts exactly these):
#   `a.b.c`                full path;                  `a.b.c` / `.d`   sibling shorthand (-> a.b.d)
#   `a.b.c` / `a.b.d`      two spans;                  `a.b.*`          "every key under a.b" (a prefix row)
#   `<svc>.replicas`       the key under every top-level service block;   `a.b[]` a list-valued key (the [] is dropped)
# Documented exceptions (not individually listed, by design) are in ALLOW_UNDER / ALLOW_LEAF below.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

VALUES=deploy/helm/aurora/values.yaml
DOCS=docs/23-helm-values.md

# `config.*` is rendered verbatim into aurora.toml: values.yaml carries a few example keys there and
# docs/15 §Configuration reference is the reference for them, not docs/23.
ALLOW_UNDER="config devIdp.resources"   # dotted prefixes; devIdp.resources: one row for the pod-resources map
# Leaves that are free-form maps/lists whose CONTENT is not enumerable (the key itself still needs a row).
ALLOW_LEAF=""
# Aurora.toml (`config`) keys that docs/23 names in prose and whose first segment happens to be a
# chart block too (`gateway`, `runner`, `secrets`, `database`, `terminal`, `counters`). They are not
# chart values. Exact keys only: a new, misspelt or removed chart key still fails.
ALLOW_NAMED="database.max_connections database.gateway_role gateway.allowed_private gateway.proxy_tls.names
  gateway.trusted_proxies gateway.insecure_loopback_injection runner.max_sandboxes runner.k8s.namespace secrets.default_store secrets.stores
  secrets.stores.id terminal.awake_cap_secs"

fail() { echo "helm-values-docs: FAIL: $*" >&2; exit 1; }
[ -f "$VALUES" ] && [ -f "$DOCS" ] || fail "$VALUES or $DOCS missing"

paths="$(awk '
  # A one-level inline map (`maxConnections: {api: 20, gateway: 10}`) is a map with leaf children.
  function flush(   n, i) {
    if (have) {
      # members of an inline map are K ("known"): they must exist if docs names them, but docs need
      # not list each one (the inline map itself is the leaf that needs a row)
      if (nkids > 0) { print "L " path; for (i = 1; i <= nkids; i++) print "K " path "." kid[i] }
      else print (has_child ? "N " : "L ") path
    }
    have = 0; nkids = 0
  }
  /^[[:space:]]*(#|$)/ { next }
  /^[[:space:]]*-/ { next }                       # list items belong to the key above (a leaf)
  {
    match($0, /^ */); ind = RLENGTH; line = substr($0, ind + 1)
    if (!match(line, /^[A-Za-z0-9_][A-Za-z0-9_.-]*:( |$)/)) next
    if (skip >= 0 && ind > skip) next             # continuation of a block scalar
    skip = -1
    key = substr(line, 1, index(line, ":") - 1)
    val = substr(line, index(line, ":") + 1); sub(/^ +/, "", val)
    if (have && ind > pind) has_child = 1         # the previous key has a child iff this one is deeper
    flush()
    while (sp > 0 && sind[sp] >= ind) sp--
    path = (sp > 0) ? spath[sp] "." key : key
    sp++; sind[sp] = ind; spath[sp] = path
    have = 1; pind = ind; has_child = 0; nkids = 0
    if (val ~ /^[|>]/) skip = ind
    sub(/[[:space:]]+#.*$/, "", val)
    if (val ~ /^\{[^{}]+\}$/) {      # one level only; a `[a, b]` list value has no `key:` and is skipped
      m = split(substr(val, 2, length(val) - 2), kv, ",")
      for (i = 1; i <= m; i++) { k = kv[i]; sub(/^ +/, "", k); if (k ~ /^[A-Za-z0-9_]+:/) { sub(/:.*$/, "", k); kid[++nkids] = k } }
    }
  }
  BEGIN { skip = -1 }
  END { flush() }
' "$VALUES")"
[ -n "$paths" ] || fail "parsed no keys from $VALUES"
leaves="$(sed -n 's/^L //p' <<<"$paths")"
all="$(sed 's/^[LNK] //' <<<"$paths")"
allowed_under() { local p; for p in $ALLOW_UNDER; do case "$1." in "$p."*) return 0 ;; esac; done; return 1; }
tops="$(grep -v '\.' <<<"$all")"
[ "$(wc -l <<<"$leaves")" -ge 50 ] || fail "parsed suspiciously few leaf keys from $VALUES ($(wc -l <<<"$leaves")): the parser is broken"

# Keys docs/23 names, in document order, minus the sections that are about other key spaces
# (§The config block: left column = aurora.toml keys; §What is not a Helm value; §Values planned...).
# Output: one "<kind> <path>" per backticked span: F full path, P prefix (`a.b.*`), S shorthand
# resolved against the last full path seen on the same line, W `<svc>.x`.
named="$(awk -v tops="$(tr '\n' ' ' <<<"$tops")" '
  BEGIN { n = split(tops, t, " "); for (i = 1; i <= n; i++) top[t[i]] = 1 }
  /^## / { skip = ($0 ~ /^## (The config block|What is not a Helm value|Values planned)/) }
  skip { next }
  {
    last = ""; s = $0
    while (match(s, /`[^`]+`/)) {
      span = substr(s, RSTART + 1, RLENGTH - 2); s = substr(s, RSTART + RLENGTH)
      gsub(/\[\]/, "", span)
      if (span ~ /^<svc>\.[A-Za-z0-9_]+$/) { print "W " substr(span, 7); continue }
      if (span ~ /^\.[A-Za-z0-9_]+$/) { if (last != "") print "S " last span; continue }
      if (span !~ /^[A-Za-z][A-Za-z0-9_]*(\.([A-Za-z0-9_]+|\*))*$/) continue
      split(span, seg, "."); if (!(seg[1] in top)) continue
      if (span ~ /\.\*$/) { print "P " substr(span, 1, length(span) - 2); continue }
      print "F " span
      last = span; sub(/\.[^.]+$/, "", last)
    }
  }
' "$DOCS")"

doc_full="$(sed -n 's/^[FS] //p' <<<"$named" | sort -u)"
doc_prefix="$(sed -n 's/^P //p' <<<"$named" | sort -u)"
doc_wild="$(sed -n 's/^W //p' <<<"$named" | sort -u)"

rc=0
# 1. every leaf is documented
while IFS= read -r leaf; do
  allowed_under "$leaf" && continue
  case " $ALLOW_LEAF " in *" $leaf "*) continue ;; esac
  grep -qxF -- "$leaf" <<<"$doc_full" && continue
  # a map-valued leaf is documented by rows for its members
  grep -qF -- "$leaf." <<<"$doc_full" && continue
  # `<svc>.replicas` documents <block>.replicas for every top-level block
  case "$leaf" in *.*) grep -qxF -- "${leaf#*.}" <<<"$doc_wild" && continue ;; esac
  # `a.b.*` documents everything under a.b
  p="$leaf"; ok=0
  while [[ "$p" == *.* ]]; do p="${p%.*}"; grep -qxF -- "$p" <<<"$doc_prefix" && { ok=1; break; }; done
  [ "$ok" = 1 ] && continue
  echo "helm-values-docs: FAIL: values.yaml key '$leaf' is not documented in $DOCS" >&2; rc=1
done <<<"$leaves"

# 2. every named key exists
while IFS= read -r k; do
  [ -n "$k" ] || continue
  allowed_under "$k" && continue
  case " $(tr '\n' ' ' <<<"$ALLOW_NAMED") " in *" $k "*) continue ;; esac
  grep -qxF -- "$k" <<<"$all" && continue
  echo "helm-values-docs: FAIL: $DOCS names '$k', which does not exist in $VALUES" >&2; rc=1
done <<<"$doc_full"
while IFS= read -r k; do
  [ -n "$k" ] || continue
  allowed_under "$k" && continue
  grep -qxF -- "$k" <<<"$all" && continue
  echo "helm-values-docs: FAIL: $DOCS names the prefix '$k.*', which does not exist in $VALUES" >&2; rc=1
done <<<"$doc_prefix"

[ "$rc" = 0 ] || exit 1
echo "helm-values-docs: ok ($(wc -l <<<"$leaves") leaf keys documented, $(wc -l <<<"$doc_full") named keys exist)"
