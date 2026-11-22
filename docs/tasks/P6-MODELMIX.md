# P6-MODELMIX: An agent's model sizes from more than one key — **proposal, spec batch (Opus) later — phase 6**

> **Status: proposal (2026-11-22).** No D-, C-, SEC- or migration numbers are taken (the spec batch takes
> them from PROGRESS's next-free note). Questions Q-P6-MIX-1..10 are open, each with a recommendation.

## Goal

Owner, 2026-11-22: an agent can hold two LLM keys (say a LiteLLM key and an Anthropic key), and its default
model can come from either. Large, Medium, Small and the subagent model, however, must come from the default
model's endpoint. The owner wants to mix them, for example Large from the Anthropic key and Medium and Small
from LiteLLM. Ideally Aurora would also translate between the Anthropic and OpenAI formats when a key's
endpoint does not speak the harness's format.

## Today

- **One endpoint for every level.** `agents.model_roles` is `{ endpoint_id, large?, medium?, small? }`
  (C-876, `03` CHECK, `ModelRoles` in `services/aurora-api/src/domains/agents/dto.rs`). It applies only while
  the turn's endpoint is that endpoint. Endpoint roles (`llm.endpoints[].model_roles`) are per endpoint.
- **The reason is written down.** D-303 and `08 §Model roles`: "Roles never name another endpoint, because a
  harness process has one base URL." `claude-code` gets one `ANTHROPIC_BASE_URL` and maps the levels to
  `ANTHROPIC_DEFAULT_{OPUS,SONNET,HAIKU}_MODEL`. The subagent model is `CLAUDE_CODE_SUBAGENT_MODEL=<alias>`
  (`12 §claude-code`).
- **How the traffic leaves the sandbox.** Each conversation's harness uses its own loopback proxy port, which
  carries the turn's context credential `auc_` (`12`, proxy table). The gateway adds the LLM key by
  destination host (`06` step 6, 11), applies the model allow-list to the request's `model`
  (`aurora-llm::model_from_request`, `06` step 6a), and records `request_log.llm_model`.
- **Two keys on one host are ambiguous.** `06` step 6 answers 409 `multiple_providers` when two granted
  candidates serve the same host, unless the request names one with `x-aurora-connection-id` (after the
  precedence rules: workspace over org scope, and so on).
- **The gateway never touches responses.** It "never parses or rewrites a response body" and never alters
  an upstream status (`08 §Phase 4 providers`, *Streaming*, *Error classification*; D-260(d)).
- **Usage** is reported by the harness as `by_model`. The API stamps `provider` and `endpoint_id` from the
  turn's single `ModelChoice` (D-304).
- **Formats per harness** (`12 §Harness interface`): `claude-code` Anthropic only (plus Bedrock and Vertex);
  `codex` OpenAI Responses only; `pi` and `opencode` all three; `goose` Anthropic and OpenAI Chat.

## Proposal

### Part 1 — Route by key (no translation)

**Unit: a key, not an endpoint (Q-P6-MIX-2).** Each level (Large, Medium, Small) and the subagent model may
name `{ secret_id, model }`: a granted LLM key of the agent and one of that key's models. This covers two
endpoints, and also two keys on one endpoint (aigw serves several LiteLLM namespaces whose model names differ).
The default model and its key are chosen exactly as today (`08 §Model selection`); that key is the **default
key**.

**Model ids the harness sees (Q-P6-MIX-3, -4).** Models of the default key keep their real ids, so an agent
with one key sees no change. A model of any other key gets the id `<prefix>:<model id>`, for example
`personal:claude-opus-4-5`, where `<prefix>` is a slug of the key's name (`[a-z0-9-]{1,32}`, made unique
among the turn's routes). The id must match `MODEL_ID_PATTERN` (`^[A-Za-z0-9._:/@-]{1,200}$`), so it can
hold no space. The dashboard shows it as "personal · claude-opus-4-5". Real model ids may contain `:` and `/`
(Bedrock `…-v1:0`, Ollama `llama3:8b`, LiteLLM `anthropic/…`). The router therefore never splits an id: it
looks the whole string up in the turn's route table. A route table in which a default-key model id equals
another route's prefixed id is refused when the agent is saved.

**The router (Q-P6-MIX-1).** For each conversation, the supervisor runs a loopback listener next to that
conversation's proxy port. The harness's base URL points at it. For each request:

1. Read the JSON body and its `model`. The body is buffered up to a fixed bound (the spec batch sets it,
   larger than any request the pinned harnesses send); a larger body is refused with 413.
2. Look `model` up in the route table. A prefixed id → that key's route, with `model` rewritten to the real
   id. Any other id, and a request without a model (`GET /v1/models`), → the default key (Q-P6-MIX-5).
3. Forward to the route's base URL **through the conversation's own proxy port**, so the request carries
   the turn credential and is attributed exactly as today. When the route's host has more than one granted
   key, set `x-aurora-connection-id: <secret_id>` (the spec batch confirms that step 6 honours it for LLM
   secrets).
4. Stream the response back untouched.

**The gateway does not change.** It still adds the key by host, checks the allow-list against the real model
id, enforces inference paths and logs `llm_model`. Nothing new becomes reachable: both keys are already
granted, and code in the sandbox can already send requests to either host with its key added. The router
runs without privileges, and a bug in it can break a turn but cannot reveal a key or get around policy.

**Model choice and adapters.** `ModelChoice` gains
`routes: [{ secret_id, endpoint_id, provider, base_url, dialect, prefix }]`. Its `roles` hold the ids as the
harness sees them. The adapters render the router's address as the base URL and render the level variables
as today. One router serves every harness (Q-P6-MIX-7). Codex declares no roles and is unchanged.

**Usage.** A `by_model` entry with a prefixed id is mapped by the API, through the turn's route table, to
`(provider, endpoint_id, real model)`. Any other id maps to the default key. The D-304 allowed set is
computed per key. The harness's claims are still never trusted for provider or endpoint.

**Readiness and drops.** Each level reports its key's readiness (`not_active`, `origin_unverified`, …) and
the role warnings against that key (`role_not_served` against that key's listing). A level whose key
becomes unusable, or is no longer granted, is dropped with a notice, never re-pointed (the override rule).

**Operator roles (Q-P6-MIX-6).** `llm.endpoints[].model_roles` stay single-endpoint. Mixing is an agent
setting only.

**Dashboard.** Instructions › Advanced (Expert, D-406): each level's select lists the models of every usable
key whose endpoint speaks the harness's format, grouped by key ("personal", "aigw-team"). The default model
select is unchanged.

### Part 2 — Translation (optional, after Part 1; Q-P6-MIX-8, -9)

When a level's key serves none of the harness's formats, the router translates the request and the response
stream between Anthropic Messages, OpenAI Chat and OpenAI Responses. That lets `claude-code` use endpoints
that speak only OpenAI (Azure OpenAI, plain vLLM), and `codex` use Anthropic.

- **Library.** NVIDIA's `switchyard-translation` crate (Apache-2.0; 0.3.0 published 2026-09-22; depends only
  on serde, futures and `switchyard-protocol`, with no HTTP or server code). It converts requests, buffered
  responses and streaming events between the three formats. It is pre-1.0, so it is pinned to an exact
  version like the harness CLIs and upgraded only by a brief.
- **On the sandbox side, by design.** The translated request reaches the gateway as an ordinary request in
  the target format and is checked as one. The gateway keeps its rule of never reading a response body.
- **Translation loses features.** Claude Code relies on Anthropic-only features: `cache_control` prompt
  caching, signed thinking blocks, `/v1/messages/count_tokens`, beta headers and server tools. These are lost
  or approximated on an OpenAI target, and losing caching alone can cost a lot. The router uses an endpoint's
  own format whenever it serves the harness's, and readiness warns `translated` on every translated level.
- **Errors.** Upstream error bodies are translated too, so that the adapters still classify them
  (`provider_quota`, `auth`, …).
- **Conformance.** One live suite run per (harness, target format) pair: streaming, tool calls, errors and
  token usage. Whether Claude Code falls back cleanly when `count_tokens` has no target equivalent is checked
  there.

### Rejected

- **A router in the gateway** (a virtual endpoint that picks the destination host from the model). For
  routing alone it would work. But the per-conversation route table would have to reach the gateway, and
  translation there would break "never reads a response body" inside the component that holds the keys.
- **An off-the-shelf proxy server** (Fastllm-proxy, traceloop hub, api7 AISIX, LiteLLM). Each holds the
  provider keys itself, while in Aurora keys never leave the gateway: origin checks, grants and the request
  log depend on the gateway adding the key by host. Each would also be another deployed service with
  configuration that changes every turn. traceloop hub does not accept Anthropic Messages at all.
- **Splitting the id on `:` to find the key.** Real model ids contain `:` and `/`.
- **Native multi-provider configuration per harness** (OpenCode `provider/model`, goose lead/worker) as the
  main mechanism: it covers two of five harnesses and not `claude-code` (Q-P6-MIX-7).

## Deliverables of the spec batch (when phase 6 starts)

- D-numbers for the answers. D-303 amended ("roles never name another endpoint" becomes "a level names a key
  and a model; one router per conversation").
- `08 §Model selection`, `§Model roles` and readiness; `12` (the router, the adapters' base URL, the
  conformance suite); `03` (`agents.model_roles` v2 with its CHECK and a migration that rewrites the v1
  shape); `05` and `API_CONTRACTS` (ModelRoles v2, readiness per level, the grouped model listing);
  `10 §Usage and metering`; `14` (the level pickers); `17` (a SEC entry: the router adds no reach, mapped
  usage); `18`.
- Proposed briefs: **P6-MIX-1** (Rust API: contract, migration, resolution, readiness, usage mapping; L),
  **P6-MIX-2** (supervisor router and adapters, conformance; M–L), **P6-MIX-3** (web pickers and readiness
  lines; M), **P6-MIX-4** (Part 2 translation; M–L).

## Open questions for the owner

| # | Question | Recommendation |
|---|---|---|
| Q-P6-MIX-1 | Where does the router run: in the supervisor (one per conversation, next to its proxy port) or in the gateway as a virtual endpoint? | **Supervisor.** The gateway stays as it is, and Part 2 is possible without the gateway reading responses. |
| Q-P6-MIX-2 | Is a level tied to a key or to an endpoint? | **A key.** It also covers two keys on one endpoint (aigw namespaces), and the gateway's key choice is then explicit. |
| Q-P6-MIX-3 | Which model ids get a prefix: (a) only other keys' models, (b) every model once the agent mixes keys, (c) only colliding ids? | **(a).** An agent with one key sees no change, and a model never changes id because a second key was added. (c) makes ids depend on what other keys happen to serve. |
| Q-P6-MIX-4 | Where does the prefix come from: a slug of the key's name computed each turn, or a stored "model prefix" field on the key? | **A slug of the name.** No new field. Renaming a key changes its prefix at the next turn only; usage is stored with real ids, so history is unaffected. |
| Q-P6-MIX-5 | A request for a model id that has no prefix and is not in the table (harness background requests, a subagent definition the agent wrote with a full id): send it to the default key or refuse it? | **The default key**, as today; the gateway's allow-list still decides. |
| Q-P6-MIX-6 | Can the operator's endpoint roles also name another endpoint's models? | **No.** Mixing is a per-agent setting; endpoint roles stay a per-endpoint default. |
| Q-P6-MIX-7 | One router for every harness, or native multi-provider configuration where a harness has it (OpenCode, goose)? | **One router.** One mechanism, one conformance suite, and Claude Code needs it anyway. |
| Q-P6-MIX-8 | Is translation (Part 2) in scope, and with `switchyard-translation` or our own code? | **Yes, after Part 1, with the pinned crate.** Routing alone already solves the LiteLLM + Anthropic case. |
| Q-P6-MIX-9 | With translation, may the **default** model be translated too (for example Claude Code entirely on Azure OpenAI), or only the levels? | **Yes, the default too**, with the `translated` warning on the agent and in the readiness view. |
| Q-P6-MIX-10 | When in phase 6? | **After P6-WARMPOOL Parts 1 and 2** (first in phase 6, D-379), independent of P6-ORGTREE and P6-LIMITS. Part 2 follows Part 1. |
