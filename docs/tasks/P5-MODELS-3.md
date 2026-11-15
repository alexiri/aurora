# P5-MODELS-3: Dashboard for default model, roles, subagent default and usage per model, plus the operator recipe (owner, 2026-11-15)

## Goal

Let a person set an agent's model roles and its subagent default, and see what the agent will really
use. Let an instance admin set roles and model details on an LLM endpoint. Show usage and request-log
models per model. Write the operator recipe for LiteLLM roles and cross-provider fallback (D-302(c)).

## Linked requirements

- D-302, D-303, D-304.
- Contracts **C-875, C-876, C-878, C-879, C-880, C-882**.
- Normative: `14` (Instructions tab, Access › Model readiness, Admin › Settings › LLM endpoints,
  Activity); `08 §Model roles`; `14 §Visual design`. Every screen follows it: tokens only, the shared
  primitives and patterns, both themes, and no horizontal scroll at 390 px.

## Context

- The Instructions tab is `web/src/components/agents/instructions-tab.tsx`. The model picker uses
  `GET /v1/agents/{a}/models`.
- Readiness is `web/src/components/agents/model-readiness.tsx`, on the Access tab.
- Admin › Settings › LLM endpoints is built by SET-3, with the P4-LLM-6 provider fields.
- The API types are `web/src/lib/api/schema.d.ts`, regenerated from the OpenAPI the backend tasks
  update.

## Scope

1. **Instructions**: the "Default model" label and help text, the **Model roles** group, **Subagents
   use**, inline warnings from readiness, and "Not used by Codex" when `/v1/harnesses`
   `model_roles = []`. The text is in `14`. Saving sends C-876 (`model_roles` with the current default
   `endpoint_id`). Map `model_role_endpoint_mismatch` and `llm_model_not_allowed` to field errors.
   For `claude-code`, each role select's label shows its alias.
2. **Access › Model readiness**: the resolved roles with their source, plus the warnings and the
   dropped roles (C-878).
3. **Admin › Settings › LLM endpoints**: **Model roles** (three fields, offering the endpoint's models
   or free text checked against `MODEL_ID_PATTERN` on the client; the server is authoritative) and
   **Model details** (a keyed list: model id → context window, capability checkboxes), with the
   no-roles `Notice` for non-Anthropic endpoints. Add the shared keyed-list primitive if SET-3's is
   missing a variant. Do not write one-off styling.
4. **Activity and usage**: the request-log `llm_model` column; tokens per model in turn details (and
   in the usage view, if it exists); the `unverified_models` marker.
5. **Operator recipe**: `docs/operator/llm-model-roles.md`. Cover:
   - a LiteLLM config serving Claude and GLM models;
   - the endpoint's `model_roles` and `model_info` for GLM (context window, capabilities);
   - an agent with `subagent_model = small` doing gather-then-write;
   - a LiteLLM model group with router fallbacks (Anthropic first) used as the default model or a
     role, with its limits stated: the upstream keys live in LiteLLM, governance and key-origin checks
     apply to the LiteLLM key only, and usage shows the group name;
   - allow-lists as the cost control.

   Link it from `docs/operator`'s index.

## Constraints / file ownership

- This task owns the files above plus any shared primitive it extends. It makes no backend change.
  If a contract is missing a field, stop and report.
- TypeScript strict. Run the repository's lint, type check, unit tests and production build. Add
  Playwright coverage where the agent-page and admin-settings suites already have it.

## Acceptance criteria

1. Setting, clearing and saving roles and the subagent default round-trips. Errors appear on the
   right field. The Codex note shows for a `codex` agent.
2. The readiness section matches the API for: an unset role on LiteLLM, a dropped role, and a role
   not allowed for the org.
3. An admin can set endpoint roles and model details. A server `value_invalid` is shown on its key.
4. Turn usage per model and request-log models render. The unverified marker renders.
5. Both themes are correct, with no horizontal scroll at 390 px. Lint, type check, tests and build are
   green, with counts reported.
6. The operator recipe exists and is linked, and its TOML and YAML examples match C-875.

## Dependencies

- P5-MODELS-1 (C-875/876/878/882) merged. P5-MODELS-2 (C-879/880) merged for scope item 4.
- SET-3 and P4-LLM-6 merged for scope item 3.

## Suggested agent/model and reason

Sonnet (`nextjs-developer`). Several screens, plus a shared primitive that may need extending.
