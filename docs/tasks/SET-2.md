# SET-2: Admin › Governance — instance governance and BYO floors as live settings — **medium, phase 5**

## Goal
The instance ceiling of `08 §11.1` (governance floors, egress baseline, key scopes, agent-token age,
BYO switch and floors) is edited on **Admin › Governance** and takes effect live in the API and every
gateway, replacing FIX-131 Part A's planned read-only view (D-195(f)). Organizations keep their own
governance exactly as today; the page shows where they stand.

## Linked requirements
- D-195(b)/(f), **D-236**, **D-237** (section `governance`, whole lists, `byo.enabled` live), D-238.
- Contracts **C-778** (governance section, `GET /v1/admin/settings/governance/orgs`), **C-783**
  (clamps name their setting), C-772..C-774 (generic section routes from SET-1).
- Spec: `15 §Instance settings` row `governance`; `08 §11.1` (instance level = application state,
  re-evaluation for every org), `08 §11.2` rules, `08 §11.9` (BYO, live switch), `08 §7` (token
  maximum age retrofit); `06 §Caches and invalidation`; `14 §Instance administration` (Settings,
  org-page links).
- Security: SEC-89, SEC-90 (from SET-1); the governance invariants of `17` stay as they are.
- Migration numbers if needed: `20261201000192..000193` (none expected).
- Depends on **SET-1** (merged). Closes FIX-131 (already marked superseded).

## Context
- API: `services/aurora-api/src/domains/governance/{service,routes,dto}.rs` (org governance
  validation against the instance, the effective view, `GET /v1/governance/instance`); BYO checks
  in the agents domain.
- Gateway: `services/aurora-gateway/src/{governance.rs,main.rs,byo.rs,resolve.rs,decide.rs}`
  (`instance_from_config` at start; `cfg.governance.byo`).
- Config: `crates/aurora-config/src/{raw.rs,validate.rs}` (`GovernanceSection`, `ByoSection`,
  `Governance`, `Byo`, `known_llm_hosts`).
- Web: org governance page (`web/src/app/o/[org]/governance/**`), the SET-1 generic section form.

## Scope
### In scope
1. **Registry entries** for section `governance` (keys exactly as `15 §Instance settings`), with
   types, defaults from `15 §Configuration reference`, whole lists (`allowed_key_scopes`,
   `known_llm_hosts_extra`, `baseline_allowlist`, `default_source_cidrs`) and loosening orders:
   `open` < `sanctioned_only`; `open` < `credentialed_only`; `enabled` < `allowlist_only` <
   `disabled`; `enabled` < `owners_only` < `disabled`; `byo.enabled` true is looser;
   `require_*` false is looser; a larger or removed `max_age_days` is looser; list supersets are
   looser for allow-lists/scopes/CIDR widening. Update `movable-keys.txt`.
2. **API readers live**: every read of these keys goes through `state.settings()` instead of
   `state.config` (org governance validation, effective view, pickers, BYO create/regenerate,
   `GET /v1/governance/instance`, agent-token mint and retrofit).
3. **Gateway readers live**: build the instance governance from the snapshot on every swap of
   `governance` (and of `llm`, which SET-3 wires — leave the hook); after a swap flush as `all`, so
   effective-governance and credential-identity caches (BYO posture, source lists, require-context)
   are recomputed. `byo.enabled` no longer needs a restart (`08 §11.9`).
4. **Re-evaluation job**: a committed `governance` change enqueues the `08 §11.2` re-evaluation for
   every organization (stored-origin compliance, dropped model overrides, owner notices, gateway
   `org` invalidation), idempotent and resumable, one org per step; tightening
   `agent_tokens.max_age_days` triggers `08 §7`'s retrofit for live tokens.
5. **Preview** (`affected_orgs`): organizations whose effective postures, lists or token age change,
   with per-key before/after (C-774), from the same computation as item 6.
6. **`GET /v1/admin/settings/governance/orgs`** (C-778): paginated, effective postures, `stricter`
   and `clamped` keys; metadata only.
7. **C-783**: `GET /v1/orgs/{org}/governance/effective`, `GET /v1/governance/instance` and BYO
   refusals' `details` carry `instance_setting: { section: "governance", key }` where the value
   comes from the instance.
8. **Dashboard**: `/admin/settings/governance` — the generic section form in four `FormSection`s
   (LLM, Egress, BYO, Agent tokens) with explanatory hints from `08 §11`; a `Notice` "Policy is set
   per organization and workspace" linking to the docs (no instance rule scope, D-195(f)); the
   **Where organizations stand** `DataTable` (from item 6) below the form; the loosening
   `ConfirmDialog` lists affected organizations. Org governance pages: "enforced by instance" links
   to `/admin/settings/governance#<key>` for instance admins, plain text for others.
9. Docs: `docs/operator/gateway-posture.md` and `limits-and-approvals.md` (dashboard first, Helm
   as the locked alternative); `23` unchanged beyond SET-1.

### Out of scope
- `llm.*` keys (SET-3); org governance semantics; an instance policy scope.

## Constraints / file ownership
Owns: `domains/governance/**`, the BYO checks it reads, gateway `governance.rs`/`byo.rs`/settings
consumer wiring, the re-evaluation job module, registry entries for `governance`,
`movable-keys.txt`, `web/src/app/admin/settings/governance/**`,
`web/src/components/admin/settings/governance-*.tsx`, the org governance page's instance links,
`web/openapi.json`, `schema.d.ts`. After SET-1; before SET-3 (SET-3 reuses the gateway rebuild and
the re-evaluation job). Shared primitives, tokens, both themes, 390 px.

## Acceptance criteria
1. IT: GET section shows every key with sources; a key set in the test config is locked (409).
2. IT: raising `governance.llm.minimum_posture` to `sanctioned_only` without restart → an org at
   `open` is effectively `sanctioned_only` (effective view, pickers) and the gateway refuses an
   agent-supplied LLM key at once (gateway IT with a NOTIFY, no restart).
3. IT: `byo.enabled = false` live → BYO create 403 `byo_disabled`, gateway refuses `aug_` with no
   restart; back to `true` restores service.
4. IT: re-evaluation job marks a now non-compliant LLM secret `mismatch`/non-permitted and drops a
   disallowed model override in every org, once (idempotent on rerun).
5. IT: preview reports loosening and the affected organizations; `…/governance/orgs` matches a
   stricter and a clamped org.
6. IT: `instance_setting` appears on the effective view's instance-sourced values (C-783).
7. IT: 412/403/audit/e-mail paths are SET-1's (one smoke test each through this section).
8. Web tests: four sections render, locked keys read-only, loosening confirm lists orgs,
   "Where organizations stand" table, org page link only for instance admins; 390 px, both themes.
9. fmt, clippy, targeted api + gateway tests, full api suite at the end, guards exit 0, web checks
   and production build.

## Suggested agent/model and reason
Sonnet (`rust-developer`, then `nextjs-developer`); Opus review of the gateway rebuild and cache
flush ordering (a stale floor would loosen enforcement).
