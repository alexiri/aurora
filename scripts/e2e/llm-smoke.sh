#!/usr/bin/env bash
# Smoke test of the VidaiMock scenarios through LiteLLM (decision I-30, TEST-017).
#   scripts/e2e/llm-smoke.sh <litellm-base-url> [master-key]
# Checks: pong (stream + JSON), streaming tool_use with non-empty fragmented input, tool_result ->
# final text, 529/429 error mapping, wrong key rejected, `claude-*` wildcard.
set -uo pipefail
BASE="${1:?usage: llm-smoke.sh <litellm-base-url> [key]}"
KEY="${2:-sk-aurora-mock-dev}"
fail=0
for _ in $(seq 90); do curl -s -m 2 -o /dev/null "$BASE/health/liveliness" && break; sleep 1; done
ok() { echo "ok   $*"; }
bad() { echo "FAIL $*" >&2; fail=1; }
post() { # model body-extra stream -> prints "status\nbody"
  curl -sS -m 30 -w '\n%{http_code}' -X POST "$BASE/v1/messages" \
    -H "x-api-key: $KEY" -H 'anthropic-version: 2023-06-01' -H 'content-type: application/json' \
    -d "{\"model\":\"$1\",\"max_tokens\":64,\"stream\":$3,\"messages\":[{\"role\":\"user\",\"content\":\"hi\"}]$2}"
}
TOOLS=',"tools":[{"name":"mcp__aurora__memory_save","description":"save","input_schema":{"type":"object","required":["key","content"],"properties":{"key":{"type":"string"},"content":{"type":"string"}}}}]'

out=$(post scn-pong "" false); [ "$(tail -1 <<<"$out")" = 200 ] && grep -q '"text":"pong"' <<<"$out" && ok "pong json" || bad "pong json: $out"
out=$(post scn-pong "" true); { grep -q 'event: message_stop' <<<"$out" && grep -q '"text":"pong"' <<<"$out"; } && ok "pong stream" || bad "pong stream: $out"

out=$(post scn-memory-tool "$TOOLS" true)
args=$(grep '^data:' <<<"$out" | sed 's/^data: *//' | jq -rs '[.[] | select(.delta.type? == "input_json_delta") | .delta.partial_json] | join("")')
if jq -e '.key == "conformance" and (.content | length) > 0' <<<"$args" >/dev/null 2>&1 && grep -q '"stop_reason":"tool_use"' <<<"$out" && grep -q '"name":"mcp__aurora__memory_save"' <<<"$out"; then
  ok "streaming tool_use with input $args"
else bad "tool_use stream (args='$args'): $out"; fi

HIST=',"messages":[{"role":"user","content":"hi"},{"role":"assistant","content":[{"type":"tool_use","id":"toolu_scn_memory_1","name":"mcp__aurora__memory_save","input":{"key":"conformance","content":"Saved by the conformance run."}}]},{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_scn_memory_1","content":"saved"}]}]'
out=$(curl -sS -m 30 -w '\n%{http_code}' -X POST "$BASE/v1/messages" -H "x-api-key: $KEY" -H 'anthropic-version: 2023-06-01' -H 'content-type: application/json' \
  -d "{\"model\":\"scn-memory-tool\",\"max_tokens\":64,\"stream\":true$TOOLS$HIST}")
{ grep -q '"text":"done"' <<<"$out" && ! grep -q tool_use <<<"$out"; } && ok "tool_result -> final text" || bad "tool_result final: $out"

# LiteLLM v1.108 relays an upstream 529 as its own 500 api_error (the overloaded_error text stays in
# the message; any upstream 5xx behaves so); 429 keeps its status.
for pair in overload:500:overloaded_error ratelimit:429:rate_limit_error; do
  IFS=: read -r id code typ <<<"$pair"
  out=$(post "scn-$id" "" true)
  { [ "$(tail -1 <<<"$out")" = "$code" ] && grep -q "$typ" <<<"$out"; } && ok "$id -> $code $typ" || bad "$id: $out"
done

code=$(curl -s -o /dev/null -w '%{http_code}' -m 10 -X POST "$BASE/v1/messages" -H 'x-api-key: wrong' -H 'content-type: application/json' -d '{"model":"scn-pong","max_tokens":8,"messages":[{"role":"user","content":"x"}]}')
case "$code" in 4??) ok "wrong key -> $code";; *) bad "wrong key gave $code";; esac
out=$(post claude-haiku-4-5 "" false); { [ "$(tail -1 <<<"$out")" = 200 ] && grep -q 'Mock model response' <<<"$out"; } && ok "claude-* wildcard" || bad "wildcard: $out"
exit $fail
