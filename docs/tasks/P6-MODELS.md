# P6-MODELS: The harness chooses models — **proposal, spec batch (Opus) later — phase 6**

> **Status: proposal (2026-11-15).** Written ahead of phase 6. Nothing here is decided yet. No D-, C-, SEC-
> or migration numbers are taken; the spec batch takes them from PROGRESS's next-free note. The open questions
> are `Q-P6-MOD-n` at the end, and the recommendations in them are the coordinator's.

## Goal

The owner wrote on 2026-11-15:

> "Different LLMs (providers and models) have varying costs and capabilities. Currently, agents are configured
> with a single LLM provider and model, and that locks in their costs."

The owner gave two use cases:

1. "use anthropic until you run out of quota, then fall back to litellm"
2. "use GLM-5.3-Flash to call tools and gather data, then Opus to write up the response"

The owner then asked: "this is a whole lot easier if we just delegate choosing models to Claude Code (or whatever
harness). do we need ModelChoice, or at least does it really need to limit Claude's options to only that
model/provider?"

The answer this proposal specifies is to keep `ModelChoice` and change what its model means. Today the agent's
model reads as the only model the agent uses. It becomes the model the harness **starts** with. The harness may
choose any model that governance allows **on the same endpoint**. Aurora tells the harness which concrete models
stand behind its aliases. Usage is then recorded per model. Use case 2 then becomes a harness feature, with Claude
Code subagents that run on a cheaper model. Use case 1 crosses endpoints, so it stays with the LiteLLM router
(*Fallback across providers*).

## Today (what already exists)

| Area | Behaviour today | Where |
|---|---|---|
| Selection | One `ModelChoice` per turn: `{ provider, endpoint_id, base_url, dialect, model, effort, client_env }`. It comes from the agent's `model_provider`/`model_endpoint_id`/`model_id`, or from auto-selection when they are null | `08 §Model selection`, `16` (`turn.deliver`), `03` (`agents`) |
| Harness environment | The `claude-code` adapter sets `ANTHROPIC_MODEL=<model>` plus the endpoint's `client_env`. It sets no alias variables. `codex` renders `model` into `config.toml` | `services/aurora-supervisor/src/adapters/claude_code/config.rs`, `12 §Adapter codex` |
| Terminal | `harness.env` carries the same `ANTHROPIC_MODEL` / `AURORA_CODEX_MODEL` | `API_CONTRACTS` C-750 (4) |
| Enforcement | The gateway checks each request's model (`model_from_request`) against the effective **allow-list** only. Nothing ties a request to the agent's chosen model | `06` step 6a, `08 §11.5` |
| Usage | The harness reports token counts. The API stamps `provider`, `endpoint_id` and `model` from `ModelChoice` and never from the harness's claims. A `by_model` aggregate is specified, but the usage route is not built yet. The `claude-code` adapter reads the result's `usage` (`stream.rs`). Per Claude Code's docs, that field covers **the main loop only**: subagent tokens appear only in the per-model breakdown (`modelUsage`) and in `total_cost_usd`. So a turn that uses subagents is **under-counted today** | `10 §Usage and metering`, `08` (*Error classification and usage*), `18` (usage test), `services/aurora-supervisor/src/adapters/claude_code/stream.rs` |
| Request log | It records no model | `03 §request_log` |
| Evals | The run fingerprint records `model_provider`, `model_endpoint_id`, `model_id` and `effort` | `10 §Evals` |

So the single model is **not a limit** today. Claude Code may already call other models on the endpoint, for
example a subagent with a `model:` field or its own small-model requests, and the gateway allows them whenever the
allow-list does. The actual limits are elsewhere:

- **One endpoint per harness process.** A Claude Code process has a single base URL, or a single Bedrock or
  Vertex configuration, and Codex has a single provider block. So the harness can choose a model but never an
  endpoint.
- **Aliases resolve to Anthropic ids.** Claude Code's `opus`, `sonnet` and `haiku` aliases resolve to Anthropic
  model ids. On a LiteLLM, Bedrock or Vertex endpoint those ids may not be served, or may not be allowed. In that
  case a subagent or a background request fails with an upstream 4xx or with `llm_model_not_allowed`.
- **Usage carries the wrong label.** Every token of a turn is recorded under the default model, including tokens
  that a subagent spent on another model.

## Proposed model

### 1. The agent's model is a default

- `agents.model_id` (and the endpoint's or the provider's default) is the model the harness **starts each session
  with**. Its main loop uses it unless the harness switches. The harness, a subagent, or an agent-written harness
  setting may use **any model the turn's endpoint serves that governance allows**. Nothing new is enforced: the
  effective allow-list (`08 §11.5`) is, and remains, the only model control. `08 §Model selection` says so
  explicitly, which replaces any reading of "the agent's model" as an exclusive choice.
- The selection rules stay unchanged. A null model still means auto-selection. An explicit model is still stamped
  with its provider and endpoint, and is dropped with a notice when it is no longer usable or allowed. The
  provider is still never inferred from a model id.
- The **endpoint** still comes only from `ModelChoice`. Nothing inside the sandbox can point the harness at another
  endpoint with the turn's key. The gateway still injects per host, and the base URL is rendered by the adapter.
- The dashboard labels the field **Default model**. The API field name does not change.

### 2. Model roles

Aurora defines three harness-neutral **roles**: `large`, `medium` and `small`. A role names a model **on the same
endpoint** as the default model.

- **Where values come from.** The most specific source wins:
  1. the agent's `model_roles` (`{ large?, medium?, small? }`, editable by whoever may edit the agent's model);
  2. the endpoint's `llm.endpoints[].model_roles` (operator configuration, also settable in Admin › LLM endpoints);
  3. nothing. The harness then uses its built-in defaults. That is correct on `anthropic` and is the usual cause of
     failures elsewhere (*Readiness* below).
- **Validation.** This happens at save time and again at every selection, using the same function as the default
  model:
  - The id is checked as an opaque string: `[A-Za-z0-9._:/@-]{1,200}`. Bedrock profile ids and ARNs and Vertex
    `model@version` ids fit this pattern. It is refused with 422 `model_role_invalid` otherwise. These values reach
    a process environment and the shell-sourceable `harness.env`, so the pattern also rules out quoting and
    whitespace.
  - The model must be on the effective allow-list when one applies, else 422 `llm_model_not_allowed`.
  - Where the endpoint has a static `models` list, the model must be on it.
  - A role that later stops being allowed is **dropped with a notice** in the next turn and never re-pointed,
    under the rule that already applies to an overridden default model.
- **`ModelChoice` gains `roles: { large?, medium?, small? }`** with the resolved model ids only. This touches the
  `16` wire (`turn.deliver`) and the terminal's `TerminalModel.choice` (C-750 (4)). A supervisor that receives no
  `roles` behaves as today.
- **Harness contract.** `HarnessCapabilities` gains `model_roles: Vec<ModelRole>`. These are the roles the pinned
  version can map, declared per version and never probed.
  - **`claude-code`** declares all three. The adapter maps them to Claude Code's alias variables:
    - `large` → `ANTHROPIC_DEFAULT_OPUS_MODEL`
    - `medium` → `ANTHROPIC_DEFAULT_SONNET_MODEL`
    - `small` → `ANTHROPIC_DEFAULT_HAIKU_MODEL`

    So `model: haiku` in a subagent definition, or the CLI's own background requests (the docs say the haiku
    variable is "also used for background functionality"), reach the role's model. The exact variable names are
    pinned next to the CLI version and verified by the live conformance suite, as the `client_env` names are
    (C-809). An unset role sets no variable. The CLI's other aliases (`fable`) are left unmapped; a request for one
    goes to the endpoint and through the allow-list as today. The terminal wrapper's `harness.env` section for
    `claude-code` gets the same variables.
  - **Subagent default.** An agent setting `subagent_model`, one of `inherit` (the default), `large`, `medium` or
    `small`, maps to `CLAUDE_CODE_SUBAGENT_MODEL=<alias of that role>`. According to the docs, this variable applies
    to every subagent whose definition names no model, below a per-call or per-definition model and above the main
    model. It does not change the built-in Explore and Plan agents. With it set to `small`, use case 2 needs no
    subagent definition at all: the general-purpose subagent runs on the cheap model. `CLAUDE_CODE_SUBAGENT_MODEL_FORCE`
    is not set, so a definition's own `model:` still wins.
  - **Model metadata for ids the CLI does not recognise.** For a gateway model id it does not know, Claude Code
    assumes a 1M-token context window and enables features by matching the id. For non-Anthropic models behind
    LiteLLM (GLM, for example) that is wrong. So the endpoint configuration may give a model's `context_tokens` and
    `capabilities`. The adapter renders them into the CLI's documented per-alias `…_SUPPORTED_CAPABILITIES` variables
    and the context-window override, with names pinned with the version. The spec batch checks against the pinned
    CLI whether the context override can be set per alias or only globally. If it is global, it is set for the
    default model only.
  - **`codex`** declares none until a pinned version can choose per-task models. The dashboard then shows roles as
    "not used by Codex", and the roles are not sent.
- **Instructions.** When roles are set, the platform preamble (`10 §Agent instructions`) gains one generated line,
  for example: "Models on this endpoint: default `claude-opus-…`; large `…`, medium `…`, small `glm-5.3-flash`. A
  subagent can use a smaller model for gathering. Write the final answer yourself." Changing a role is an
  instruction-input change, so the session is released while idle and the next turn starts it again with the same
  `resume_ref` (the existing rule).
- **Subagent definitions** (`.claude/agents/*.md`) are agent-owned files in the home and are not projected by the
  platform (Q-P6-MOD-4). The guide pattern for use case 2 is a `gatherer` subagent with `model: haiku`, so it runs on
  `small`, and tools enabled. The main loop on the default model delegates the tool-heavy gathering and writes the
  answer. This is the shape that saves money: every step of a tool loop resends the growing context, so the loop
  belongs on the cheap model, and the expensive model reads condensed results.

### 3. Usage per model

- **Adapter.** `HarnessEvent::Usage` and `Done { usage }` carry `by_model: [{ model, input_tokens, output_tokens,
  cache_read_tokens, cache_write_tokens }]` next to the totals. `claude-code` fills the breakdown from the result's
  per-model report (`modelUsage`, which per the docs is keyed by model id with `inputTokens`, `outputTokens`,
  `cacheReadInputTokens` and `cacheCreationInputTokens`, and includes subagents). The exact key in the raw
  stream-json output is pinned with the CLI version. **The totals are then the sum of the breakdown**, not the
  main-loop `usage`, which fixes today's under-count. The CLI's own `costUSD` estimates are ignored: Aurora records
  no money (D-276(b)). A harness without a breakdown reports totals only, and those are attributed to the default
  model, as today.
- **Trust rule.** The API keeps stamping `provider` and `endpoint_id` from `ModelChoice`, because one turn means one
  endpoint. The `model` of each entry is accepted only when it is the default model, a resolved role, or on the
  effective allow-list. The gateway refuses every other model wherever an allow-list applies, so a model outside
  that set cannot have been used there. An entry that fails the check is folded into the default model's entry, and
  the turn's usage is marked `unverified_models: true`. When no allow-list applies, every claimed model id that
  passes the role-id pattern is accepted, and this is said in `17`. Token counts were always harness-reported, and
  this does not change. The trust rule protects which model a cost is attributed to; it does not verify the counts
  themselves.
- **`turns.usage`** becomes `{ totals…, provider, endpoint_id, model (the default), by_model: [...],
  unverified_models? }`. The specified `by_model` aggregate of the usage route (D-260(g)), which is not built yet,
  is keyed by `(provider, endpoint_id, model)` over these entries instead of over the turn's single model.
- **Gateway cross-check.** `request_log` gains `llm_model text null`. It holds the `model_from_request` result for
  every request on a governed LLM endpoint where a key was injected (`Some(id)`, null otherwise). It is used for
  audit and for comparing claimed against observed models in Admin views. The API's trust rule never reads it, so
  log write timing cannot affect usage.

### 4. Readiness and dashboard

- `GET /v1/agents/{agent}/model-readiness` adds `roles` to its `default`: each role's resolved model and source
  (`agent` / `endpoint` / `unset`) plus a **warning**:
  - `role_unset_non_anthropic`: the endpoint is not `anthropic`, and the role is unset. The harness falls back to
    its built-in Anthropic ids, which the endpoint may not serve.
  - `role_not_allowed`: the role was dropped.

  Warnings never fail a turn. Only `no_model_key` and `no_default_model` do.
- **Agent page, model section.** It shows **Default model** (today's picker), and under it **Model roles**: three
  pickers limited to the same endpoint's allowed models, each with "Use the endpoint's setting", plus **Subagents
  use**, which offers *same as the default* or one of the three roles, and the warnings. For Codex it shows "Not
  used by Codex".
- **Admin › LLM endpoints** gets the three role fields.
- **Usage** (turn details, and the workspace usage view once it exists) breaks tokens down by model.
- **Evals.** The run fingerprint (`10 §Evals`) gains the resolved `model_roles`, so a comparison shows a role change
  as a field-level difference.

### 5. Fallback across providers (use case 1)

A harness cannot switch endpoints (*Today*). Claude Code's own `--fallback-model` / `fallbackModel` does not cover
this case either. According to its docs, it triggers only when the model is overloaded, unavailable, or returns a
non-retryable server error. It never triggers on rate-limit, billing or quota errors, and it stays on the same
endpoint. (It could be offered later for resilience against overload; that is not in scope here.) The supported
answer is therefore the **LiteLLM router**: the operator
defines a model group, for example `smart`, with Anthropic first and the fallbacks after it, and points the agent's
default model, or a role, at that group. `docs/operator` gets a recipe. That costs Aurora nothing. Its limits are
stated in the recipe:

- the upstream keys live in LiteLLM, not in Aurora's secret store;
- governance and key-origin checks apply to the LiteLLM key only;
- usage shows the group name, not the provider that answered.

An Aurora-native, governance-visible **model chain** remains possible later (Q-P6-MOD-3). It would be an ordered
list of default selections, and the switch to the next one would happen at a turn boundary when a turn ends
`provider_quota` or `provider_rate_limited`. The entry that failed would cool down, the session would restart
through release and `resume_ref`, and the failed turn would be **continued** on the next entry, never replayed. It
is not part of this proposal.

### Security notes (for `17`)

- Roles and the default model carry model ids only, never credentials. They are validated against the pattern
  above, both before they enter `client_env`/`harness.env` and at the API (the supervisor re-checks them).
- The allow-list remains the model boundary. A model the harness picks, from a role, a subagent, or a harness
  setting the agent wrote in its home, gets no more than an allowed model on the turn's endpoint with the turn's
  key. The endpoint and the key never come from inside the sandbox. This is the intended trust model and is stated
  explicitly.
- Cost control stays outside Aurora: allow-lists, plus LiteLLM per-key budgets and model lists (`21`: budgets are
  out of scope, D-276(b)). Operators who want "no Opus for this team" use the allow-list or LiteLLM.

### To verify against the pinned CLI before the spec batch

The Claude Code facts above come from its documentation, checked on 2026-11-15 (code.claude.com: model-config,
env-vars, sub-agents, cli-reference, headless, costs, agent-sdk cost-tracking). The pinned version is 2.1.319. Run
the pinned binary through a logging proxy, which is what `request_log.llm_model` provides once it exists, and
confirm:

- (a) The raw stream-json `result` carries the per-model breakdown, and under which key.
- (b) Which background requests use the haiku alias in `-p` mode with `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1`
  (session recap, idle compaction, prompt suggestions are named in the docs). Today these may already be hitting
  unmapped Anthropic ids on LiteLLM endpoints.
- (c) Whether the context-window override can be set per alias.
- (d) That `ANTHROPIC_DEFAULT_*_MODEL` with a LiteLLM model name works through `ANTHROPIC_BASE_URL`.

## Deliverables of the spec batch (when phase 6 starts)

Owning-doc changes:

- `08`: §Model selection (default semantics, roles, resolution, dropping), §Model readiness (role warnings), §11.5
  (the allow-list as the only model boundary), *Error classification and usage* (trust rule)
- `10`: §Agents (model fields), §Agent instructions (roles line), §Usage and metering (`by_model`), §Evals
  (fingerprint)
- `12`: §Harness interface (`model_roles` capability, `ModelChoice.roles`), §Adapter `claude-code` (alias
  variables), §Adapter `codex` (no roles)
- `03`: `agents.model_roles` jsonb null, `request_log.llm_model`, `turns.usage` shape
- `05`: agent fields, the readiness response, usage
- `16`: `ModelChoice.roles`
- `15`: `llm.endpoints[].model_roles`
- `14`: the model section, Admin › LLM endpoints, usage by model
- `17`: SEC, per the security notes above
- `18`: tests
- `API_CONTRACTS`: `ModelChoice`, C-750 amendment, readiness
- `docs/operator`: LiteLLM fallback and roles recipe

The implementation should land **after P4-LLM-5**, because it touches the usage stamping and the adapters that
P4-LLM changes. Implementation briefs, roughly:

| Brief (proposed) | Scope | Size |
|---|---|---|
| P6-MOD-1 | Roles end to end, backend side: config `model_roles` (plus optional per-model `context_tokens`/`capabilities`) and `agents.model_roles` and `agents.subagent_model` (migration), resolution and validation in `model::select`, `ModelChoice.roles` on the wire and in `TerminalModel`, the `model_roles` capability, `claude-code` alias, subagent-default and metadata variables (adapter and `harness.env`), the instructions line, role readiness warnings, the eval fingerprint; live conformance: a subagent with `model: haiku` reaches the `small` model, and an unset role on LiteLLM produces the warning | M, Rust |
| P6-MOD-2 | Per-model usage: `claude-code` per-model parsing with totals summed from it (fixes the subagent under-count), the trust rule, the `turns.usage.by_model` shape, `request_log.llm_model` (migration), the `by_model` aggregate in the usage route (or its spec, if the route is still unbuilt). The under-count fix alone could be pulled forward as a FIX (Q-P6-MOD-7) | M, Rust |
| P6-MOD-3 | Dashboard: Default model label, role pickers on the agent and in Admin › LLM endpoints, readiness warnings, per-model usage in turn details; the operator recipe | M, Next.js |

Order: P6-MOD-1, then P6-MOD-2. P6-MOD-3 can start once P6-MOD-1's contract is merged. The work is independent of
P6-ORGTREE and P6-LIMITS. An `llm.requests` cap in P6-LIMITS still counts requests, whatever the model.

## Open questions for the owner

- **Q-P6-MOD-1** Should the agent's model become a **default**, with the harness free to use any model governance
  allows on that endpoint, and the allow-list as the only model control? *Recommended: yes.* This is already true in
  effect today; the change states it, makes it usable (roles), and makes usage honest.
- **Q-P6-MOD-2** Role names: harness-neutral `large`/`medium`/`small`, mapped by each adapter, or Claude's
  `opus`/`sonnet`/`haiku` directly? *Recommended: neutral.* Codex or a later harness maps them its own way, and
  the dashboard shows the Claude alias next to each role for `claude-code`.
- **Q-P6-MOD-3** Fallback when a provider runs out: should the LiteLLM router (recipe only) be the answer for now,
  with an Aurora-native model chain switching at turn boundaries only if governance has to see each provider's key?
  *Recommended: yes, LiteLLM now, and the chain later only on demand.*
- **Q-P6-MOD-4** Should Aurora ship or manage subagent definitions, for example a platform `gatherer` on `small`,
  or a dashboard editor for `.claude/agents`? Or should they stay agent-owned files that a person or the agent
  writes? *Recommended: agent-owned for now, plus a guide paragraph and the instructions line. Revisit once
  per-model usage shows whether agents delegate well.*
- **Q-P6-MOD-5** Should an agent have its own model allow-list, narrower than governance's, for example "this agent
  may not use Opus"? *Recommended: not in the first cut.* Governance allow-lists and LiteLLM per-key model lists
  already cover it, and a third layer complicates readiness.
- **Q-P6-MOD-6** Should this wait for phase 6 (D-278), or be placed at the end of phase 5 right after P4-LLM-5,
  since it is small and touches the same code? *Recommended: the owner decides. Technically it fits right after
  P4-LLM-5.*
- **Q-P6-MOD-7** Today's `claude-code` usage leaves out subagent tokens (*Today*). Should the totals fix, which sums
  the per-model breakdown, be pulled forward as a phase-5 FIX after verification (a) above, independent of the rest
  of this proposal? *Recommended: yes.* It is a correctness bug in recorded usage, and a small one.
