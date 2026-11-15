# P5-MODELS-1: Model roles, subagent default and model metadata, backend (owner, 2026-11-15)

## Goal

The owner asked, on 2026-11-15: "does it really need to limit Claude's options to only that
model/provider?" The answer (D-302) is that the agent's model becomes the harness's **default**. The
harness may use any model governance allows on the same endpoint. **Model roles** (`large`, `medium`,
`small`) name those other models, so that Claude Code's aliases resolve to real models on LiteLLM,
Bedrock and Vertex endpoints. This task builds the backend side: configuration, the agent fields,
resolution in the dispatcher, the readiness report, the wire, the `claude-code` environment (turns and
terminal), the instructions line and the eval fingerprint.

## Linked requirements

- D-302 (owner answers), D-303 (design); SEC-122 (model ids are inert data), SEC-123 (models, never
  endpoints or keys).
- Contracts **C-875, C-876, C-877, C-878, C-881, C-882, C-883, C-884**.
- Normative: `08 §Model selection`, `§Model roles`, `§Model readiness`; `10 §Agents` (model fields),
  `§Agent instructions` (*Models*), `§Evals` (fingerprint); `12 §Harness interface`, `§Adapter
  claude-code`, `§Adapter codex`; `16` (`ModelChoice`); `03` (`agents.model_roles`,
  `agents.subagent_model`); `05` (agent route, `/v1/harnesses`, readiness); `15` (`llm.endpoints[]`);
  `02 §Limits` (`MODEL_ID_MAX`, `MODEL_INFO_MAX`); `18 §Model roles and usage per model`.

## Context

Today the adapter sets only `ANTHROPIC_MODEL`
(`services/aurora-supervisor/src/adapters/claude_code/config.rs`). The gateway checks requests only
against the allow-list, so Claude Code's subagents and background requests already use other models.
On a non-Anthropic endpoint, though, they ask for built-in Claude ids that the endpoint may not serve.
Claude Code (pinned 2.1.319) documents the variables `ANTHROPIC_DEFAULT_{OPUS,SONNET,HAIKU}_MODEL`,
`CLAUDE_CODE_SUBAGENT_MODEL` (applies to subagents whose definition names no model; not to
Explore/Plan), the per-alias `…_MODEL_SUPPORTED_CAPABILITIES`, and `CLAUDE_CODE_MAX_CONTEXT_TOKENS`
(code.claude.com: model-config, env-vars, sub-agents; checked 2026-11-15). **Verify every name
against the pinned binary.** The live suite is the evidence (acceptance 6).

## Scope

1. **Limits and pattern** (`crates/aurora-core/src/limits.rs`): `MODEL_ID_MAX = 200` and
   `MODEL_INFO_MAX = 64`. Remove their `doc-only` lines from `scripts/guards/limits-gaps.allow`.
   Add `MODEL_ID_PATTERN` with one validator in `crates/aurora-llm` (`validate_model_id`) that the
   config, the API and the supervisor all use.
2. **Endpoint configuration** (`crates/aurora-config`, `crates/aurora-llm/src/endpoint.rs`): the
   `model_roles` and `model_info` keys of C-875, with their validation and `value_invalid` refusals.
   Keep the key names clear of the credential-name check (`context_window`, not `…token…`). Settings
   writes of `llm.endpoints` (C-779) validate the same way.
3. **Migration and agent fields** (`services/aurora-api`, `domains/agents`): add
   `agents.model_roles jsonb null` and `agents.subagent_model text not null default 'inherit'`, with a
   CHECK. Take the migration number from PROGRESS's next-free line when you start, and edit the line.
   Add GET/PATCH fields and errors per C-876, plus audit. A PATCH is a render-input change, and the
   existing release-while-idle path handles it.
4. **Resolution** (`services/aurora-api/src/domains/hosted/model.rs`, the function that already
   serves the dispatcher and readiness): resolve the roles per `08 §Model roles`:
   - The source order is an agent role stamped for this endpoint, then an endpoint role, then none.
   - An allow-list check applies at selection.
   - Drop an agent role per role, clearing it in the database, with one next-turn notice through the
     existing override-drop notice path. Never re-point it.
   - An endpoint role that is not allowed gives `role_not_allowed` and is not sent.
   - `subagent_role` follows its rule.
   - `model_meta` holds the default model and the resolved roles only.

   Readiness (C-878) returns the same data, from the same call.
5. **Wire** (`crates/aurora-protocol/src/runner.rs` `ModelChoice`): add the optional `roles`,
   `subagent_role` and `model_meta`, each with `skip_serializing_if`, so older peers are unaffected.
   Add the C-750 `TerminalModel.choice` fields (C-881).
6. **Harness contract** (`services/aurora-supervisor/src/harness.rs`): add
   `HarnessCapabilities.model_roles`. `claude-code` declares all three roles; `codex` and `fake`
   declare none. The API's `/v1/harnesses` returns `model_roles` (C-882) from the API-side harness
   registry. Keep it consistent with the supervisor's declaration through a shared constant in
   `aurora-protocol` or a test.
7. **`claude-code` environment** (`adapters/claude_code/config.rs` and `harness_env.rs`): map the
   variables per C-877 and write them single-quoted into `harness.env`. The supervisor re-checks every
   value and refuses the turn with `harness_failed` on a violation (SEC-122). Never set
   `CLAUDE_CODE_SUBAGENT_MODEL_FORCE`, and never write under `.claude/agents/`.
8. **Instructions line** (`domains/hosted/home.rs` render inputs, plus the supervisor's renderer):
   add the C-884 fragment.
9. **Eval fingerprint** (C-883): if P4-EVAL's fingerprint (C-866) is merged when you start, add
   `model_roles` and `subagent_model` to it. Otherwise leave a one-line note in PROGRESS, so that
   whichever of P4-EVAL-2 and this task merges second adds them. Do not edit P4-EVAL files on another
   lane's branch.
10. **Docs**: if the pinned CLI disagrees with C-877 (a variable name, or a per-alias context
    override that does or does not exist), update `12 §Adapter claude-code` and C-877 in the same
    commit, with a one-line note under D-303.

## Constraints / file ownership

- This task owns `domains/hosted/model.rs`, the `agents` model fields, `aurora-llm` endpoint
  validation, the `claude-code` config and env code, `harness_env.rs`, and `ModelChoice` in
  `aurora-protocol`.
- Do not touch the usage parsing (`adapters/claude_code/stream.rs` result handling,
  `hosted::set_usage`). That belongs to P5-MODELS-2, which runs after this task in the same lane.
- No dashboard work; that is P5-MODELS-3.
- Rust conventions as in `02`. Use `cargo fmt --check`, clippy, and targeted tests per step. The full
  suites run at the batch end (lane practice).

## Acceptance criteria (each test seen failing first)

1. Configuration and settings refusals: a bad id, an unknown role, an unknown capability, a role not
   on `allowed_models` or on the static `models` list, and more than `MODEL_INFO_MAX` entries each give
   `value_invalid` naming the key.
2. Agent PATCH: the C-876 errors, the audit row, and the roles stamped with the default endpoint.
   `model_role_endpoint_mismatch` is returned when the stamp does not match.
3. Selection and readiness (`api/it/model_roles.rs`): every case listed in
   `18 §Model roles and usage per model` *Selection*, with the readiness result equal to the
   dispatcher's choice.
4. Golden `claude-code` environments: no roles; all roles; subagent default; capabilities and context
   window; `codex` ignoring roles; a `harness.env` with roles that `sh -n` accepts and that sources back
   to the same values.
5. SEC-122: ids with quotes, `$()`, newlines, spaces or `=`, an id of 201 characters, and
   credential-shaped values are each refused in config, in PATCH, in selection and by the supervisor.
6. Live conformance (env-gated, a LiteLLM endpoint that serves two models, wired like the existing
   `claude-code` live suite):
   - a subagent defined with `model: haiku` and the CLI's background requests reach `roles.small`,
     as seen in the gateway request log (use `request_log.llm_model` once P5-MODELS-2 lands; until
     then, the mock or LiteLLM log);
   - the main loop uses the default model;
   - with `subagent_model = small`, the general-purpose subagent uses `small`.

   Record the observed behaviour and the variable names in the adapter's pinned notes.
7. The `/v1/harnesses` items carry `model_roles`. Session release happens on a role change.
8. `cargo fmt --check`, clippy, and the limits guard are green. Report test counts.

## Dependencies

- **P4-LLM-5** merged. This task edits the same adapter environment and the provider `client_env`.
- C-779 (settings `llm.endpoints`) as merged with SET-3. If SET-3 has not merged, validate the TOML
  path only, and note that SET-3 must call the same validator.

## Suggested agent/model and reason

Sonnet (`rust-developer`). It is multi-crate but well bounded, with a live-suite step that needs
judgement about the pinned CLI's real behaviour.
