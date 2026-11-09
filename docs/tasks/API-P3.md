# API-P3: Phase-3 contracts before parallel work

## Goal
Write the `docs/API_CONTRACTS.md` entries that phase-3 lanes implement, so no two lanes invent the same API shape (CLAUDE.md: "define/update contracts before parallel backend/frontend work").

The **normative spec already exists** (SPEC-RFE-P3, D-59..D-67, SEC-52..SEC-58, phase-tagged "(phase 3, RFE-nnn)" across the owning documents). This task **consolidates** it into numbered contract entries. It doesn't invent behaviour: where the spec is silent on a wire detail, propose the smallest choice, mark it "(proposed)", and list it in the report for the coordinator.

## Contract blocks (assigned)
- RFE-013 agent-token max age and use tracking: **C-310..C-319**
- RFE-018 `Idempotency-Key` on message creation: **C-320..C-329**
- RFE-020 workspace-tier skills: **C-330..C-339**
- RFE-022 plugin major upgrade (`impact`, `reauth_required`): **C-340..C-349**
- RFE-023 simulate code `default_rule_skipped_llm_host`: **C-350..C-359**
- RFE-024 self-contained deprovision (op `credentials` on the connector wire): **C-360..C-369**
- Vault store (configuration and reference scopes): **C-370..C-379**
- RFE-014 gateway under a Postgres outage (`readyz` states, refusal codes, metrics) and RFE-019 (defaults; probably no wire contract): use C-380..C-389 if needed.

## Sources (read only what each section needs)
- `docs/80-design-review.md` §RFE-013, 014, 018, 019, 020, 022, 023, 024 (owner decisions) and `docs/90-decisions.md` D-59..D-67.
- The phase-3 tagged text in `docs/05` (routes), `docs/06` (gateway, outage), `docs/08` (secret stores, tokens), `docs/09 §10` (plugin upgrades), `docs/10` (skills, idempotency), `docs/13` and `docs/16` (connector wire, ops), `docs/14` (dashboard), `docs/15` (config), `docs/17` (SEC-52..SEC-58), `docs/18` (test sections), `docs/21 §Phase 3 — HashiCorp Vault`.
- Find them with `grep -n "phase 3" docs/*.md`.
- The shape and style of existing entries in `docs/API_CONTRACTS.md` (e.g. the phase-2 C-2xx sections).

## Also in scope
- **Vault configuration keys in `docs/15 §Configuration`** (`secrets.stores[]` for the `vault` kind: address, auth `kubernetes` / `approle` / `token`, mount, path templates, renewal, `reference_scopes`), from `21` and `08 §2`. Each key gets a default and a validation rule.
- **The Q12 invariant** (owner: yes): a new **SEC-61** row in `docs/17` covering Vault reference mode: reference scopes enforced, latest version only, no cross-org path, a reference never readable outside its scope. Add a matching pending row in `docs/tests/security-coverage.md` that names the proving test P3-VLT-1 must write.
- The OpenAPI surface stays code-generated. Contracts describe routes, methods, auth, request/response shapes, errors, pagination, nullability, IDs and timestamps.

## Constraints
- **Owns:** `docs/API_CONTRACTS.md` (new phase-3 sections only), `docs/15` (Vault keys), `docs/17` (SEC-61 row), `docs/tests/security-coverage.md` (one pending row).
- No code.
- No change to existing phase-0..2 contracts. If one must change, report it rather than editing it.
- Decision number D-107 only for a real choice.

## Acceptance criteria
- [ ] Every phase-3 route, wire field, error code and config key named in the spec has a contract entry or a config row. List any spec text you couldn't map.
- [ ] Each entry cites its spec source (doc §section, RFE, D-number).
- [ ] `bash scripts/guards/run-all.sh` green (no conflict markers; schema guard untouched).
- [ ] A short report of the "(proposed)" choices for the coordinator to confirm.
