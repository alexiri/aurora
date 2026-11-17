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

## Completion report (lane `p5-apps`, 2026-11-17)

Commits (branch `alexiri/p5-apps`): `e0a56426` (1 Instructions), `f5982aaf` (2 readiness), `32fac38f` (3 endpoint
dialog), `4dd72b94` (4 request log, `TurnUsage`), `10850cc7` (5 operator recipe), `a4e185bb` (Playwright),
`eaba4f57` (fixture fix found by the batch gate).

**Where the brief was adapted to master**
- Pages: the endpoint dialog lives under Administration › AI models › Endpoints (`/admin/ai-models?tab=endpoints`,
  P5-IA-3). No navigation entry was added. The dialog is a custom control inside the settings form, not a P5-IA-5
  field row, so it needed no field metadata.
- Scope 3 "shared keyed-list primitive": SET-3 has none for a keyed list *inside* a dialog, so **Model details** is
  composed from `Field`, `CheckboxField`, `Button` and the dialog's own fieldset pattern
  (`web/src/components/admin/settings/llm-model-fields.tsx`); no one-off classes.
- Scope 4: **no turn-details surface and no usage view exist yet** (`Turn.usage` is rendered nowhere; P5-USAGE-2 builds
  the `UsageView`). Delivered: the request-log **Model** column (`llm_model`, `activity-view.tsx`) and
  `web/src/components/chat/turn-usage.tsx` (`TurnUsage`: tokens per model, a turn without `by_model` counted under
  `model`, the `unverified_models` marker with the text from `14`), unit-tested but **not mounted**. P5-USAGE-2 should
  mount it (or reuse its rows) in the agent Computer page's usage card and the per-model aggregate (C-879 route
  belongs to P5-USAGE-1). Acceptance 4 is therefore met for the request log and the marker component only.
- Roles use `GET /v1/agents/{a}/model-readiness` for the default endpoint and the warnings (owners only); a non-owner
  sees the group without warnings. `llm_model_not_allowed` / `model_id_invalid` map by `details.field`
  (`model_roles.large`, `subagent_model`, else the Default model field); `model_role_endpoint_mismatch` shows on the
  group. A server `value_invalid` on an endpoint appears on its field (`model_roles`, `model_info`) — the server names
  the field, not the model id inside it.
- Changing the Default model to another endpoint empties the role selects (roles never cross endpoints, D-303) and
  the save sends `model_roles: null`.
- No backend, `package.json` or lockfile change. The recipe's two TOML blocks were checked with a throwaway test
  against `aurora_config::endpoint_field_errors` (passed, then deleted).

**Gates:** `cargo clippy --workspace --all-targets --all-features -- -D warnings` ok; `bash scripts/guards/run-all.sh` ok
(exit 0); `python3 scripts/extract-schema.py --check` ok; `openapi_matches_checked_in` ok and `pnpm -C web gen:api` no
diff; no Rust crate touched, so no crate suites; web `typecheck` ok, `lint` 0 errors (5 warnings in files not touched),
`vitest` **138 files / 2272 tests passed** (the first run found 4 failures from a stale harness fixture, fixed in
`eaba4f57`), `build` ok; Playwright mocked **1170 passed** (incl. stage4-axe 258, stage4-routes 204, ia-navigation 60,
agent-page-p5 14; both themes, 390 px), production project **11 passed**. Fail-before: the new vitest tests exercise
components that did not exist, so they fail without the change.
