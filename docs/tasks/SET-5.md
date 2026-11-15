# SET-5: Operational settings — agents and sandboxes, approvals, retention, skills, SCIM, previews, terminal, limits — **large (two parts), phase 5**

## Goal
The remaining movable configuration becomes live instance settings on their own Admin › Settings
pages: agent and hosted defaults and deadlines, approvals, retention and attachment ceilings, the
git skill library, SCIM, preview and terminal caps, API rate-limit tuning, `gateway.allowed_private`
and the gateway's body and header-timeout limits. Sandbox sizes and `max_sandboxes` stay in Helm and
are shown read-only (D-235(a)). The terminal and SCIM switches become dashboard switches gated by
the deployment's wiring (D-237(d)).

## Linked requirements
- D-235(a), **D-236**, **D-237** (sections `agents`, `approvals`, `retention`, `skills`, `scim`,
  `previews`, `terminal`, `limits`; `*.available`; whole and keyed lists), D-238.
- Contracts **C-782** (operational sections: runners read-only, `available`,
  `setting_requires_deployment`, keyed rate classes, `opens_private_destinations`), **C-783**
  (retention/attachment/SCIM clamps name their setting), C-772..C-774.
- Spec: `15 §Instance settings` rows for these sections, `15 §Configuration reference` (each key's
  default and rule, `terminal.available`, `scim.available`), `23 §Instance settings and Helm locks`
  (chart rules), `23` rows `scim.ratePerMinute`, `terminal.enabled`, `scim.enabled`;
  `05 §Conventions` (rate classes), `06` (destination guard, body cap), `07` (approval timeouts),
  `10` (retention, attachments, skills), `11` (terminal, previews, start deadline), `04 §SCIM`;
  `14 §Instance administration` (Settings).
- Security: SEC-89, SEC-90; SEC-06 (`allowed_private` only widens what an admin lists), SEC-72
  (SCIM off ⇒ 404), SEC-80/SEC-87 unchanged.
- Migration numbers if needed: `20261201000198..000199` (none expected).
- Depends on **SET-1**. Independent of SET-2..SET-4; one lane, after them in practice (same nav).

## Context
- Config: `crates/aurora-config/src/{raw.rs,validate.rs}` (`AgentsSection`, `HostedSection`,
  `ApprovalsSection`, `RetentionSection`, `AttachmentsSection`, `SkillsLibrarySection`,
  `ScimSection`, `PreviewsSection`, `TerminalSection`, `ApiRateSection`, `RateClassOverride`,
  `GatewaySection`), cross-section rules (approval max vs turn ceiling, awake caps vs background cap,
  default harness ∈ enabled).
- API readers of `state.config.{agents,hosted,approvals,retention,attachments,skills,scim,previews,
  terminal,ratelimit}` across `domains/**` and `jobs.rs` (e.g. `domains/terminal/{routes,service}.rs`,
  `domains/runner_plane/{service,boot}.rs`, `domains/instance/service.rs`, previews, scim, hosted,
  retention sweeps); the API limiter (`http/**` rate limiting).
- Gateway: destination guard (`guard.rs`), body cap and header timeout (`pipeline.rs`,
  `upstream.rs`), approval timeouts (`hold.rs`).
- Chart: `deploy/helm/aurora/templates/configmap.yaml` (terminal mirroring, `scim.rate_per_minute`
  injection, `scim.enabled`), `values.yaml` (`scim.ratePerMinute`), `scripts/guards/{chart-ops.sh,
  settings-chart.sh}`.

## Scope
### In scope
**Part A — agents, approvals, retention, skills, previews, limits**
1. **Registry** entries for `agents`, `approvals`, `retention`, `skills`, `previews`, `limits`
   (keys exactly as `15 §Instance settings`; `hosted.enabled_harnesses` and
   `gateway.allowed_private` whole lists; `ratelimit.api.classes` keyed by class). Loosening: longer
   deadlines and caps, higher quotas and rates, `hosted.enabled` true, added harnesses, added
   `allowed_private` entries (warning `opens_private_destinations`). `approvals.park.*` only when
   P4-011B has landed (otherwise not registered). Update `movable-keys.txt`.
2. **Readers live**, each reading `state.settings()` (API) or the gateway snapshot at use:
   - `agents`: defaults for workspaces and agents created afterwards; deadlines for turns and
     sandbox starts begun afterwards; values a sandbox gets at boot on its next boot.
     `hosted.enabled_harnesses` ⊆ union of harnesses registered runners report (422
     `harness_not_available`); `extra.runners` (C-782) lists each runner's sizes, `max_sandboxes` and
     harnesses read-only.
   - `approvals`: per hold in API and gateway.
   - `retention`: per sweep; a lowered ceiling clamps org values (tighten-only) and the preview lists
     affected orgs.
   - `skills`: per refresh and per fetch.
   - `previews`: per use (switch and domain stay configuration).
   - `limits`: API limiter swaps bucket parameters, keeping bucket state; gateway guard list, body
     cap and header timeout for new requests; the API's app-instance probes read `allowed_private`
     from the same snapshot.
3. **C-783** for retention and attachment ceilings and SCIM domains.

**Part B — terminal and SCIM switches, chart**
1. Config: `terminal.available` and `scim.available` (configuration-only, default `false`, not in
   the registry). Registry: `terminal.*` and `scim.*` (`scim.allowed_email_domains` whole list);
   `terminal.enabled` / `scim.enabled` default to the `available` value; turning one on while not
   available → 422 `setting_requires_deployment` (C-782).
2. Readers live: every `config.terminal.enabled` / `config.scim.enabled` read becomes the live
   setting. Runners register the `terminal` capability whenever they can serve it; the API checks
   the live switch at each open and at boot-document time, so switching off closes new opens at once
   (open sessions end at their next wake check, as for a disabled agent).
3. Chart: `terminal.enabled` renders `terminal.available` instead of `config.terminal.enabled`
   (the disagreement checks become: `config.terminal.enabled: true` with `terminal.enabled: false`
   still fails the render); `scim.enabled` renders `scim.available`; `scim.ratePerMinute` defaults
   to unset and renders `config.scim.rate_per_minute` only when set. `chart-ops.sh` half-enabled
   checks and `settings-chart.sh` updated; `23` rows rewritten from "(phase 5 …) becomes" notes to
   the current state; `values.yaml` comments.
4. Upgrade check: an install whose values set `terminal.enabled: true` keeps the terminal on after
   the upgrade (default follows `available`), an install that set `config.terminal.enabled` keeps it
   locked.

**Part C — dashboard and docs**
1. Pages `/admin/settings/{agents,approvals,retention,skills,scim,previews,terminal,limits}` with the
   generic form, grouped `FormSection`s and hints from the owning spec sections; Agents & sandboxes
   shows the runners' sizes as a read-only `DataTable` ("Set in Helm: `runner.sandbox_*`");
   Terminal and Directory sync show a `Notice` when the deployment does not provide the wiring;
   Limits shows rate classes as a keyed list and `allowed_private` with a `danger` hint.
2. Docs: `docs/operator/{limits-and-approvals,retention-and-quota,skill-libraries,scim-provisioning,
   previews,terminal,deployment-defaults,harnesses}.md` (dashboard first; Helm as the locked
   alternative).

### Out of scope
- Moving runner configuration, `previews.enabled`/`domain`, `push.*`, `blobs.*` (D-237(b)).
- Approval parking itself (P4-011B).

## Constraints / file ownership
Owns: registry entries for the eight sections, the readers named in Context, gateway guard/body/
timeout wiring, chart `configmap.yaml` terminal/SCIM rendering, `values.yaml` (`scim.ratePerMinute`
and comments), `chart-ops.sh`, `settings-chart.sh`, `docs/23` rows, `web/src/app/admin/settings/
{agents,approvals,retention,skills,scim,previews,terminal,limits}/**`, matching components, the
operator docs above, `web/openapi.json`, `schema.d.ts`, `movable-keys.txt`. Part A then B then C
with targeted tests between and full suites at the end. After SET-1; run after SET-2..SET-4 in the
same lane (admin nav, registry file).

## Acceptance criteria
1. Unit: every startup validation case of these keys fails a PUT with 422 and the same key path;
   cross-section rules (approval max vs turn ceiling, awake caps vs background cap, default harness)
   name the other key.
2. IT: lowering `hosted.turn_ceiling_secs` below `approvals.max_timeout_secs + 900` → 422 naming
   `approvals.max_timeout_secs`; raising `hosted.idle_stop_secs` applies to the next idle check.
3. IT: `enabled_harnesses` with a harness no runner reports → 422 `harness_not_available`; runner
   sizes appear read-only and cannot be written (400 `setting_unknown`).
4. IT: `ratelimit.api.multiplier` change applies without restart (429 threshold moves); a class
   override in the test config locks that class only.
5. Gateway IT: adding an `allowed_private` entry live lets a request to that private address
   through; removing it refuses again; preview flags `opens_private_destinations`.
6. IT: `terminal.enabled` off live → open 404 `terminal_disabled`; on while `terminal.available`
   is false → 422 `setting_requires_deployment`; the same for SCIM (404 for `/scim/v2` when off).
7. Chart: default render contains no movable key (`settings-chart.sh`); `terminal.enabled: true`
   renders `terminal.available = true` and the wiring; `chart-ops.sh` half-enabled checks pass;
   `helm-values-docs.sh` passes.
8. Web tests per page (locked, editable, notices, runners table, keyed rate classes); 390 px, both
   themes.
9. fmt, clippy, targeted api + gateway tests, full suites at the end, guards exit 0, web checks and
   production build.

## Suggested agent/model and reason
Sonnet (`rust-developer` for A/B, `nextjs-developer` for C); Opus review of `allowed_private` and the
terminal/SCIM gating (they open network paths).

## Handoff notes (lane `p5-apps`, 2026-11-15) — NOT STARTED; findings from reading the code
Nothing of SET-5 is implemented. What the next worker should know (so the first hour is not spent rediscovering it):
- **Pattern to copy (SET-2/3/4):** registry entries as `fn <section>_defs()` in `crates/aurora-config/src/settings.rs` (`SettingDef::new(section, key, SettingType, default).loosening(..).whole_list()/.keyed(..)`); the section list already names `agents, approvals, retention, skills, scim, previews, terminal, limits` (titles only, no defs yet). A live snapshot cell in `AppState` (`app.rs`: `install_live_sign_in`/`_governance`/`_secrets` use `self.settings.on_swap(Arc::new(move |old, new| ..))`, accessor `state.auth()`, `state.llm()`, `state.gov_instance()`), per-section hooks in `domains/settings/sections.rs` (`extra`, `affected`), per-section rules in `domains/settings/{rules,service,sign_in}.rs`.
- **Do not register a key before its reader is live:** a saved value that nothing reads is a lie in the UI. Register and convert readers section by section (suggested order: `approvals` (5 files), `retention` + `attachments` (`conversations/lifecycle.rs`, `attachments/{quota,storage}.rs`), `previews` (8 files), `hosted`/`agents` (9 files, most of them), `limits` (`app.rs` limiter; gateway `guard.rs`/`pipeline.rs`/`upstream.rs`), then Part B `terminal`/`scim` (5 files each) + chart).
- **Reader sites to convert** (`state.config.<x>` today, `grep -rn "config\.<x>\b" services/aurora-api/src services/aurora-gateway/src`): `hosted` 23 hits in `domains/{agents,conversations,hosted,runner_plane}/…`; `approvals` 6 in `domains/{approvals/notify,policy/{validate,simulate},grants/{effective,service}}.rs`; `retention` 2 + `attachments` 8 in `conversations/lifecycle.rs`, `attachments/{quota,storage}.rs`; `scim` 13 in `app.rs`, `domains/scim/{rate_limit,auth,resource_users,management}.rs`; `previews` 13 in `hosted/home.rs`, `previews/service.rs`, `runner_plane/{service,work}.rs`, `connectors/managed.rs`, `http/csrf.rs`, `relay/{service,quota}.rs`; `terminal` 8 in `runner_plane/{service,work,boot}.rs`, `terminal/{routes,service}.rs`; `ratelimit` 1 in `app.rs`. Gateway: `guard.rs:147` (`allowed_private`), body cap / header timeout in `pipeline.rs`/`upstream.rs`, approval timeouts in `hold.rs`.
- A simple shape that avoids touching every call site twice: one `LiveOps` snapshot (`Arc<LiveOps>` holding the cloned `agents/hosted/approvals/retention/attachments/skills/scim/previews/terminal/ratelimit` sections of the merged `RawConfig`, rebuilt in an `install_live_ops` swap hook) and `state.ops().<section>` in place of `state.config.<section>`; one load per request path like `state.auth()`.
- Other open points: `approvals.park.*` only when P4-011B has landed (check `grep -rn "park" crates/aurora-config/src/raw.rs`); chart work (`configmap.yaml`, `values.yaml`, `chart-ops.sh`, `settings-chart.sh`, `docs/23`) is Part B; `movable-keys.txt` gets every new key; the web pages need `controls` per section only for runners table (`extra.runners`), the keyed rate classes and the `allowed_private` danger hint — the rest renders in the generic `SettingsSectionForm` with `groups`.
- **Owner hold (2026-11-15, relayed by the coordinator):** the admin navigation may be reorganized into topic pages (People & sign-in, AI models, Apps, Chat platforms, Internet access, Agents, Data). So: build the section pages as specified, but **add no sidebar link per section and no new top-level Admin/org nav entry** (ignore the "a link per section as SET-2..SET-5 land" comment in `app-shell.tsx`); sections stay reachable from the `/admin/settings` index; keep every section page component self-contained (own data, no assumption about its parent layout) so it can be embedded in a topic page later.
- Remember the batch gate 3 owed for SET-4 is being run as a slice on this lane (see the status report); SET-5 needs its own gate at its end.
