# SET-1: Instance settings framework — storage, Helm locks, validation, live reload, audit, export/import, CLI, Admin › Settings shell — **large, phase 5**

## Goal
Instance-wide settings become application state (owner, D-195(b)–(d)): stored in the database, edited
by instance admins, applied live, audited, with a value set in Helm locked. This brief builds the
**generic mechanism** and the empty Admin › Settings shell; the sections and their live readers come
in SET-2..SET-5 and FIX-124. Nothing changes behaviour for an existing install (D-238(a)).

## Linked requirements
- Decisions **D-236** (framework), **D-237** (classification, chart rules), **D-238** (upgrade,
  break-glass); owner answers D-195, D-235.
- Contracts **C-770..C-777, C-784** (`docs/API_CONTRACTS.md` §Phase 5 — SPEC-SETTINGS).
- Spec: `15 §Instance settings` (normative: effective value, mirroring, validation, live application,
  secrets, writes, break-glass, the section table), `03 §Planned changes` `instance_settings`,
  `04 §Instance admins` + `§Audit` (Instance settings row), `05 §Event bus` (`aurora_settings`),
  `05 §Instance administration` (settings rows), `06 §Caches and invalidation` (settings row),
  `08 §1` (`instance_setting` purpose), `14 §Instance administration` (Settings) + page pattern
  `SettingField`, `23 §Instance settings and Helm locks`, `02 §Limits registry` (`SETTINGS_*`).
- Security: **SEC-89**, **SEC-90**.
- Migration block: **`20261201000190`** (this brief; `000191` spare for it). SET-2..SET-5 hold
  `000192..000199`.

## Context
- Config loading: `crates/aurora-config/src/{load.rs,raw.rs,validate.rs}` (`RawConfig`,
  `api_config()`, `gateway_config()`); env overrides are merged in `load.rs`.
- API state: `services/aurora-api/src/app.rs` (`AppState.config: Arc<ApiConfig>`); CLI subcommands
  in `services/aurora-api/src/main.rs` (pattern: `secrets_migrate.rs`).
- Gateway: `services/aurora-gateway/src/{main.rs,invalidation.rs}` (LISTEN loop, `all` flush).
- Audit: `crates/aurora-core/src/audit.rs` (`SECURITY_CRITICAL_ACTIONS`),
  `services/aurora-api/src/audit.rs` (`commit_critical`).
- Re-auth: `services/aurora-api/src/domains/me/service.rs` (`REAUTH_WINDOW_SECS`, reused by
  `domains/scim/management.rs`).
- Mailer: `services/aurora-api/src/domains/auth/mail.rs`.
- Sealing: `aurora-secrets` `SecretVault::seal/open/discard`.
- Web: `web/src/components/app-shell.tsx` (`ADMIN_LINKS`), `web/src/app/admin/**`,
  re-auth step in `web/src/components/account/delete-view.tsx`, primitives in
  `web/src/components/ui`.

## Scope
### In scope
**Part A — registry and effective view (`aurora-config`)**
1. `settings` module: the registry (C-770) — sections, keys, type, default, `helm_key`, list kind,
   `secret`, `loosening`. SET-1 registers **no section's keys** except a test-only section behind
   `#[cfg(test)]`; each later brief adds its own entries. Reserve the 13 section ids.
2. **Presence**: `load()` records which dotted keys the file and `AURORA__…` overrides set (a
   `BTreeSet<String>`, keyed-list entry ids included), without changing `RawConfig`'s parsing.
3. `effective(raw, presence, stored) -> (merged RawConfig, sources)` implementing
   helm > stored > default with whole-list and keyed-list rules; then the **existing** validation
   functions run on the merged `RawConfig` (no second copy of any rule).
4. `deploy/helm/aurora/movable-keys.txt` + a unit test asserting it equals the registry, and that no
   key on D-237(b)'s list (dev/test switches, roots, `auth.instance_admins`) is in the registry.

**Part B — storage and service (API)**
1. Migration `20261201000190_instance_settings.sql` exactly as `03 §Planned changes` (table, CHECKs,
   gateway `SELECT` grant through `migrate`'s grant list). Move the table from §Planned changes into
   a real `###` section of `03` and the grant into §Gateway database role grants in the same change;
   regenerate `spec_schema.json`.
2. Settings service: load all rows, build the snapshot (`ArcSwap`) and expose
   `state.settings()`; nothing reads it yet except the shell routes.
3. **Helm mirroring** at start under `pg_advisory_xact_lock` (D-236(c)): for each locked key whose
   stored value differs, write it, bump the generation, best-effort `instance_settings.helm_sync`.
4. **Start-up validation**: invalid merged view → refuse to start naming section, key and the reset
   command (D-238(c)); rows with an unknown section or unknown keys → warning, ignored.
5. Routes C-771..C-776 (`domains/settings/**`): GET index, GET section, PUT (full replace of
   dashboard values, `$secret` handling, CAS on `generation`, 409 `setting_locked` /
   `stale_revision`, 412 re-auth via the shared check, 413, 422 `settings_invalid` with
   `details.fields`), preview (diff, loosening from the registry, `affected_orgs` hook that sections
   implement — empty by default), export (YAML/JSON, `export` class), import (dry run / apply in
   one transaction, `replace`/`merge`, `locked_ignored`, `secret_in_import`).
6. Secrets: seal into the **inline** store with purpose `instance_setting`, org nil UUID, owner = a
   new uuid per field (D-236(g)); discard replaced refs after commit; never in responses, export,
   audit, logs or e-mail.
7. Audit (C-777): `instance_settings.update|import|reset` added to `SECURITY_CRITICAL_ACTIONS` and
   committed via `commit_critical` in the write transaction; `helm_sync` best-effort.
8. After commit: `NOTIFY aurora_settings {section, generation}`; e-mail every non-disabled instance
   admin (key names only).
9. Reload: a LISTEN consumer in every API replica + `SETTINGS_RELOAD_POLL` (30 s) poll + reload all on
   reconnect; a failing reload keeps the snapshot (`aurora_settings_reload_failed_total`).
10. CLI `aurora-api settings show|reset|export|import` (C-784) through the same service; starts with
    invalid stored settings.
11. Limits: add the five `SETTINGS_*` constants to `crates/aurora-core/src/limits.rs` with readers,
    and remove their `doc-only` lines from `scripts/guards/limits-gaps.allow`.

**Part C — gateway**
1. Load the sections it reads at start, merge with its own configuration the same way, and keep a
   snapshot reloaded on `aurora_settings`, by poll and on LISTEN reconnect; flush as `all` after a
   swap of `governance`, `llm`, `limits`, `secrets`. No consumer yet (SET-2/3/5 and FIX-124 wire them).
2. A gateway with no grant on the table yet (rollout order) logs and keeps running on its
   configuration.

**Part D — chart (D-237(c)/(d), only what SET-1 needs)**
1. `values.yaml`: drop `config.hosted.enabled`; movable keys only as comments.
2. `scripts/guards/settings-chart.sh` (added to `run-all.sh`): `helm template` with default values
   (and with `relay.enabled`/`terminal.enabled`/`scim.enabled` on) and fail when the rendered
   `aurora.toml` contains a key of `movable-keys.txt`. The list grows as each section brief
   registers its keys, and each keeps the guard green in its own change (SET-5 switches the
   terminal and SCIM renderings to `*.available` in the change that registers those keys).
3. `deploy/kind` values: keep only the movable keys dev genuinely needs, so the dashboard path is
   exercised in kind.

**Part E — dashboard shell**
1. Admin nav: a **Settings** group (index only until sections land); `/admin/settings` index per
   `14` (sections, counts, last change, the locked-keys `Notice`, Export, Import… dialog with the
   dry-run diff and Apply).
2. `SettingField` primitive in `web/src/components/ui` (documented in `14`'s pattern table) and a
   `SettingsSectionForm` that renders a C-772 section generically (scalar, enum, bool, whole list,
   keyed list via `ItemList` + `FormDialog`, secret write-only field), with Save → preview →
   `ConfirmDialog` on loosening → PUT; 409/412/422 handling per `14`.
3. Generated types: `UPDATE_OPENAPI=1 cargo test -p aurora-api --test it openapi_matches_checked_in`,
   then `cd web && pnpm gen:api`.

**Part F — docs**: `docs/operator/upgrades.md` (break-glass: CLI from a running pod or a debug pod
when the API cannot start), release-note text for the locked-keys list.

### Out of scope
- Any section's keys, pages or live readers (SET-2..SET-5, FIX-124).
- Changing org or workspace layering; instance policy rules (D-195(f)).

## Constraints / file ownership
Owns: `crates/aurora-config/src/settings*.rs` + `load.rs` presence, new
`services/aurora-api/src/domains/settings/**`, `main.rs` CLI wiring, `app.rs` state, migration
`000190`, `services/aurora-gateway/src/settings.rs` + LISTEN wiring, `deploy/helm/aurora/values.yaml`
(config block only), `movable-keys.txt`, `scripts/guards/settings-chart.sh`, `run-all.sh`,
`web/src/app/admin/settings/**`, `web/src/components/admin/settings/**`,
`web/src/components/ui/setting-field.tsx`, `app-shell.tsx` (admin nav), `web/openapi.json`,
`web/src/lib/api/schema.d.ts`, `docs/03`, `docs/14`, `limits.rs`, `limits-gaps.allow`.
Sequential: FIX-124 and SET-2..SET-5 start after this merges (same nav, registry and service).
Shared primitives only, tokens only, both themes, no horizontal scroll at 390 px.

## Acceptance criteria
1. Config unit tests: presence from file and from `AURORA__…`; helm > stored > default; whole-list
   and keyed-list locking; a cross-section rule failing on the merged view names the other key.
2. Registry test: `movable-keys.txt` equals the registry; no D-237(b) key is movable (SEC-89).
3. IT (test section): GET shows sources and locks; PUT round-trip; absent key → default; locked key
   → 409 `setting_locked`; stale generation → 409 `stale_revision`; two concurrent PUTs → one 409;
   keyed test list: a config entry stays locked beside dashboard entries, and a dashboard entry
   reusing its id → 422 `entry_id_in_helm` (C-773).
4. IT: 412 without a re-authentication within 600 s; 403 for an org owner, an API key and a
   non-admin; failing audit insert → 503 and the row unchanged (SEC-90).
5. IT: a secret set via `$secret` is sealed inline, `keep` keeps it, absent clears it; a canary
   value appears in no response, export, audit row, log line or e-mail.
6. IT: mirroring — a key in the config is mirrored at start; restarting without it keeps the value
   as `dashboard`; `helm_sync` row written.
7. IT: export → import (dry run diff, apply) round-trips; `locked_ignored`; `secret_in_import`;
   one transaction (a failing section rolls back all).
8. IT: start with an invalid stored value refuses naming the key; `aurora-api settings reset`
   fixes it and is audited `instance_settings.reset` with source `cli`.
9. Gateway test: NOTIFY reload swaps the snapshot; an invalid generation keeps the old one and bumps
   the metric; poll picks up a missed notify.
10. Chart guard passes on defaults and fails when a movable key is added to `values.yaml`.
11. Web tests: index with locked-keys notice; generic section form (locked vs editable vs default,
    reset to default, secret field, keyed list); loosening confirm; 409/412/422 paths; 390 px and
    both themes.
12. `cargo fmt --check`, clippy, targeted ITs + full api suite at the end, gateway tests,
    `bash scripts/guards/run-all.sh` exit 0, web typecheck/lint/tests/production build.

## Suggested agent/model and reason
Sonnet (`rust-developer` for Parts A–D, `nextjs-developer` for E); Opus review of mirroring, the
lock enforcement and the CAS/transaction boundaries (security and data integrity).
