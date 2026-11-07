# VidaiMock for Aurora (decision I-30)

A scripted stand-in for an Anthropic-API model, used behind the **real LiteLLM proxy** so that the
live `claude-code` conformance suite and the hosted e2e run the real Claude Code CLI end to end.
**It is not a model**: every answer is a fixed scenario file. Aurora ships no first-party mock.

| | |
|---|---|
| Upstream | https://github.com/vidaiUK/VidaiMock |
| Version | **v0.4.0** (image tag `0.4.0`) |
| Image | `ghcr.io/vidaiuk/vidaimock@sha256:c306a4e87637976bfd9034d0275ae6932497b18ee71614c46a0f143f393127c6` (multi-arch index, linux/amd64 + arm64; pinned by digest, never by tag) |
| License | Apache-2.0 (upstream `LICENSE`) |
| Run as | `--config-dir /config --isolated`: only the providers in this directory are served |

Upgrading: pick a new tag, read its changelog, take its index digest
(`skopeo inspect --no-tags docker://ghcr.io/vidaiuk/vidaimock:<tag>`), change it in
`deploy/dev/llm-mock/{llm-mock.yaml,vidaimock.Containerfile}`, `scripts/e2e/llm.sh` and here, and run `bash scripts/e2e/llm.sh`.

## Layout

```
providers/anthropic.yaml               POST /v1/messages: scenario playback + plain default answer
providers/anthropic_count_tokens.yaml  POST /v1/messages/count_tokens
templates/aurora/                      default (non-scenario) answers and error envelope
scenarios/anthropic/<id>.{sse,body,status}
```

Why our own provider and templates: the bundled Anthropic template emits tool input `{}` and stops
calling tools once any `tool_result` is in the history. Claude Code needs real arguments, so tool
turns are **scripted scenarios**, not generated.

## Scenario selection

LiteLLM (`deploy/dev/llm-mock/litellm.yaml`) has one model alias per scenario, `scn-<id>` →
`anthropic/scn-<id>` on `http://vidaimock:8100`. The model name reaches VidaiMock in the request body
and `providers/anthropic.yaml` renders the scenario id from it (`scenarios.key`). A model that does
not start with `scn-` gets the plain "Mock model response." answer. An unknown `scn-` id returns a
500 naming the scenario, so a mis-wired test fails loudly. Selection never reads the message text.

Tool scenarios come in pairs, chosen by whether the request history holds a `tool_result`
(`has_tool_result`, a VidaiMock template function): `<id>-tool` is the tool call, `<id>-tool-final`
the answer after it.

| Model alias | Files | Behaviour | Used by |
|---|---|---|---|
| `scn-pong` | `pong` | streams/returns `pong` | conformance `say`/resume/correction; hosted e2e |
| `scn-count` | `count` | 40 numbered lines, 250 ms apart | conformance abort (`long`) |
| `scn-memory-tool` | `memory-tool`, `memory-tool-final` | text, then `tool_use` `mcp__aurora__memory_save` with input `{"key":"conformance","content":"Saved by the conformance run."}` streamed as fragmented `input_json_delta`; after the `tool_result`: `done` | conformance `tool`; hosted e2e |
| `scn-steer` | `steer` | slow six-word answer (400 ms apart) for the steer cases; `claude-code` has `steer = false`, so the suite does not use it yet | reserved |
| `scn-ratelimit` | `ratelimit` | HTTP 429 `rate_limit_error` | conformance `fail` → `provider_rate_limited` |
| `scn-overload` | `overload` | HTTP 529 `overloaded_error` | smoke only |
| `claude-mock`, `claude-*` | none | plain text answer | Claude Code's small/fast model calls |

Observed LiteLLM (v1.108.2-lts) behaviour: a 429 keeps its status; **any upstream 5xx, 529 included,
comes back from LiteLLM as its own 500 `api_error`** (the `overloaded_error` text survives only in the
message). So `provider_overloaded` classification of 503/529 cannot be driven through LiteLLM; it is
covered by the adapter's unit tests. An invalid key gets 400 `No connected db` from LiteLLM without
a database.

## Scenario file format (VidaiMock v0.4.0)

Files are Tera templates. `.sse` is for streaming requests and is sent event by event (blank-line
separated); `.body` is for non-streaming requests and for any error status; `.status` holds the HTTP
status. `:vidai-delay <ms>` before an event waits before sending it. Docs: upstream
`mkdocs/docs/streaming.md#scenario-playback`.

VidaiMock reads its configuration at start: restart the container after editing a file.
