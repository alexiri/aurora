# SPEC-SETTINGS: Instance settings live in the app; Helm keeps only what needs different Kubernetes configuration — **spec batch (Opus), then implementation briefs**

## Goal
Owner, 2026-11-13: "why is instance governance only at the helm level? I'd like to move as many config
options as possible from helm to the app, except for those that require genuinely different kubernetes
configuration."

Today every instance-wide setting is TOML configuration (`crates/aurora-config`, `15 §Configuration
reference`) rendered from Helm values (`23`), read at startup; the dashboard shows a few of them read-only
(Identity) and owns exactly one instance setting (`instance_placement`, D-140). The role split came from the
original spec (`08 §11.1`: the instance ceiling belongs to the *operator*); no decision record requires it.
This batch specifies **instance settings as application state**: stored in the database, edited by
instance admins in the dashboard, applied live, audited — with Helm reduced to infrastructure, bootstrap
and security roots.

## Classification of today's configuration (proposal for the owner's review)

**Stays in Helm** — needs different Kubernetes objects, is a secret/key root, or is needed before the
database is reachable:
| Section | Why |
|---|---|
| `env_secrets` (all keys, tokens, `DATABASE_URL`, OIDC client secrets as env, Redis credentials) | secret roots; rotation procedures (`key-rotation.md`) |
| `database.*`, `server.bind`, `server.trusted_proxies`, `server.external_url`, `api.*_internal_url`, `hookhost.*` | topology / ingress / needed before the DB |
| `gateway.*` binds, `ca_dir`, `proxy_tls`, advertised/sandbox proxy URLs, `trusted_proxies`, `egress_proxy` | Services, certificates, network path |
| `runner.*` backend, image, binds, `k8s.*`, `podman_socket`, `sandbox_network`, `sandbox_hosts`, `stream_tls` | pod specs, namespaces, runtime class, images |
| `relay.*`, `connector_host.*` listen/URLs, `previews.enabled` + `previews.domain` | separate Deployments, wildcard DNS/ingress |
| `secrets.stores[]` definitions (Vault address/auth) | Vault auth uses the pod's service account / AppRole |
| `ratelimit.counters.backend` + Redis, `events.delta_bus` | infrastructure choice |
| `tls.extra_ca_bundle`, `plugins.dir`/`builtin_dir`, `telemetry.*` | mounts / platform |
| Dev and test switches: `auth.dev_idp`, `hosted.allow_fake_harness`, `gateway.insecure_loopback_injection`, `plugins.allow_unsigned`, `runner.dev_*`, `connector_host.test_types` | must never be switchable from a browser session |
| `auth.instance_admins` (as a **bootstrap / break-glass** list only) | recovering access when the DB settings lock everyone out |

**Moves to the app** (database, Admin pages, live):
| Section | Admin page |
|---|---|
| `governance.llm.*`, `governance.egress.*` (minimum postures, key scopes, key-origin requirement, known LLM hosts extra, baseline allow-list, open-lane minimum), `governance.agent_tokens.max_age_days` | Governance (replaces FIX-131 Part A's read-only view) |
| `byo.*` (master switch, minimum posture, default CIDRs, context credentials) — becomes live, no rolling restart (`08` line "read from configuration at start" amended) | Governance |
| `llm.enabled_providers`, `provider_preference`, `llm.endpoints[]` (base URL, models, allow-lists, key origin, allowed paths) | LLM endpoints |
| `auth.signup_mode`, `org_creation`, session idle/absolute, OIDC providers incl. role/group mappings and JIT (client secret sealed in the secret store; a Helm-defined provider remains possible and is marked "managed by Helm") | Sign-in (today's read-only Identity becomes editable) |
| `email.*` (SMTP URL sealed in the secret store, from address) | Email |
| `secrets.default_store`, reference scopes (store *definitions* stay in Helm) | Secret stores (FIX-124 becomes partly editable) |
| `agents.*` defaults, `hosted.*` defaults and timeouts, `enabled_harnesses` (only among the harnesses the runner image reports) | Agents & sandboxes |
| `approvals.*`, `retention.*`, `attachments.*` quotas, `skills.library.*`, `scim.*`, previews quotas/caps, `terminal.*` caps and `enabled` | per-feature sections |
| `ratelimit.api.*` (multiplier, org bucket, class overrides, preview rates), `gateway.allowed_private`, gateway body/time limits | Limits (`allowed_private` is security-critical: it opens private address egress) |

**Undecided, listed for the owner**: sandbox resource sizes and `max_sandboxes` (they are runtime pod-spec
values, not Helm-rendered objects, but interact with namespace quotas) — propose: move, validated against
the runner's reported limits.

## Design points the batch must specify
1. **Storage**: typed settings documents per section (`instance_settings(section pk, value jsonb, generation,
   updated_by, updated_at)`), validated by the same rules as `aurora-config/src/validate.rs` on the merged
   view; CAS on `generation`; invalid → 422 per field. `03`, migration plan.
2. **Precedence (Q-FB-5)**: recommended — Helm values **seed** a section the first time it is absent; after
   that the database wins; Helm may list **locked** keys (`settings.locked: ["governance.*", …]`) which the
   dashboard shows read-only with "managed by Helm" and the API refuses to change. Alternative: any value
   set in Helm always wins (GitOps-first). Either way the effective value and its source are visible.
3. **Live application**: every moved setting takes effect without restart: API replicas and the gateway
   reload via the existing invalidation path (snapshot generation / notify, `06`, `07`); list today's
   startup-only readers and how each becomes live; sandboxes get changed values on next boot where the
   value is a boot input (`restart_to_update`, as today).
4. **Security**: changes are **security-critical audited** with a before/after diff (`04 §Audit`, FIX-125's
   Admin › Audit shows them), require a recent re-authentication (`auth_time`, FIX-108), notify every
   instance admin; loosening a ceiling shows the affected orgs before saving. Secrets entered in the UI
   are sealed (`08 §2`), never returned.
5. **Org/workspace layering unchanged**: `effective = instance ∩ org`, `max(instance minimum, org)` (`08
   §11.1`) — only the instance level's storage moves; "enforced by instance" links to the setting.
6. **Export/import**: `GET /v1/admin/settings/export` (YAML/JSON, no secrets) and an import with a diff
   preview, so the owner can still keep settings in Git.
7. **Helm/`23` and `15` rewritten**: each moved key marked "seed only (edit in Admin › …)" or removed;
   `deployment-defaults.md` updated; upgrade path for existing installs (first boot of the new version
   seeds from current values — no behaviour change on upgrade).

## Deliverables
- Spec changes: `08 §11.1` (instance level = application state), `15`, `23`, `03`, `04 §Audit`, `05`
  (`/v1/admin/settings/{section}` GET/PUT, export/import), `14 §Instance administration` (Settings area
  with sections), `17` (new/amended SEC invariant: dev switches never DB-editable; locked keys enforced
  server-side), decisions (next free D).
- Implementation briefs, sequenced: **SET-1** framework (table, validation, CAS, reload, audit, lock,
  export/import, Admin › Settings shell); **SET-2** Governance + BYO; **SET-3** LLM endpoints; **SET-4**
  Sign-in/OIDC + email (sealed secrets); **SET-5** operational limits (agents, hosted, approvals,
  retention, attachments, skills, SCIM, previews/terminal caps, rate limits, `allowed_private`).
  FIX-131 Part A and the editable part of FIX-124 fold into SET-2/SET-1.

## Owner questions
Q-FB-5 (precedence), Q-FB-6 (scheduling: phase 4 now vs phase 5), and the classification table above.

## Suggested agent/model and reason
Opus main session (architecture + spec reconciliation); implementation by Sonnet lanes per SET brief.
