# Aurora — Implementation Specification

Aurora is a self-hosted platform for running AI agents **as a team**: every
agent gets its own identity, the credentials and apps it was explicitly
granted, and (optionally) a sandboxed computer. The agent's only route to the
outside world is a **gateway** that authenticates each request, evaluates
policy, can hold it for human approval, and only then injects the real
credential. Agents never hold real secrets.

Aurora follows the design of OneCLI (documented in `../onecli-spec/`) and
adopts the agreed proposals from `../onecli-spec/proposals/`:

- **App plugin system** (Proposal 01): apps are signed, declarative packages
  with optional sandboxed WebAssembly hooks; the core contains no app-specific
  code.
- **Channel connectors** (Proposal 02): chat platforms are separate connector
  services speaking one provider-neutral contract; the core contains no
  platform names. Includes the per-turn **context credential**.
- **Memory language** (Proposal 03): one memory language per workspace.

This repository is the **normative specification** that implementation agents
build from. It is not a description of an existing system.

## Ground rules for implementers

- **Normative language.** MUST / MUST NOT / SHOULD / MAY carry their RFC 2119
  meaning. Anything not marked is descriptive context.
- **One source of truth per fact.** Each fact (a table, a wire message, an
  error code, a constant) is defined in exactly one document; other documents
  link to it. If you find a contradiction, the *owning* document wins and the
  contradiction is a spec bug to report.
- **Owning documents:** tables → `03`; HTTP routes → `05`; gateway behaviour →
  `06`; policy semantics → `07`; secret storage, LLM providers and LLM/egress governance → `08`;
  plugin manifest, WIT and hook-host API → `09`; connector contract → `13`;
  runner and supervisor wires → `16`;
  constants and limits → `02 §Limits registry`; security invariants → `17`.
- **Phases are real.** Every feature carries a phase tag. Do not implement a
  later-phase feature early unless the plan in `19` says so; do implement the
  *seams* phase 0 requires for later phases (they are called out explicitly).
- **Fail closed.** Wherever the spec is silent about an error path, the safe
  default is: refuse, log, and surface a typed error.

## Technology summary

| Concern | Choice |
|---|---|
| All services, CLI, supervisor, connectors, plugin SDK | **Rust** (edition 2024, stable toolchain) |
| Web dashboard | **Next.js** (App Router, TypeScript) — a pure API client |
| Database | **PostgreSQL** (16+) — system of record, work queue, event bus (`LISTEN/NOTIFY`) |
| Plugin hooks | Rust compiled to **WebAssembly components**, run by wasmtime in the hook host |
| Dev deployment | **podman compose** |
| Production deployment | **Kubernetes** via a **Helm** chart |

## Reading order

| # | Document | Contents |
|---|----------|----------|
| 00 | [Overview and glossary](00-overview.md) | Product, mental model, vocabulary, what changed vs OneCLI |
| 01 | [Architecture](01-architecture.md) | Components, planes, trust boundaries, end-to-end flows |
| 02 | [Engineering conventions](02-engineering-conventions.md) | Repository layout, crates, libraries, coding rules, limits registry |
| 03 | [Data model](03-data-model.md) | Every table, column and constraint |
| 04 | [Identity and tenancy](04-identity-tenancy.md) | Users, orgs, workspaces, roles, sessions, OIDC, API keys, access law |
| 05 | [Control-plane API](05-control-plane-api.md) | API server, event bus, due-work engine, full route catalogue |
| 06 | [Gateway](06-gateway.md) | Proxy lanes, request pipeline, injection mechanisms, guidance, caches |
| 07 | [Policy and approvals](07-policy-approvals.md) | Rule model, evaluation, grants, approvals, action approvals, agent links |
| 08 | [Secrets and credentials](08-secrets-credentials.md) | Pluggable secret stores, inline encryption, LLM keys, OAuth refresh, gateway credentials |
| 09 | [App plugin system](09-app-plugins.md) | Package format, manifest contract v1, mechanisms, hooks, snapshot, lifecycle, SDK/CLI |
| 10 | [Agents and conversations](10-agents-conversations.md) | Agents, conversations, turns, transcript, memory, skills, schedules, webhooks, watches, peers |
| 11 | [Runner and sandboxes](11-runner-sandboxes.md) | Runner, backends (podman, kubernetes, fake), sandbox lifecycle, image |
| 12 | [Supervisor and harness](12-supervisor-harness.md) | In-sandbox supervisor, harness interface, platform tools, home projection |
| 13 | [Channels and connector contract](13-channels-connectors.md) | Neutral channel core, connector contract v1, capabilities, invariants |
| 14 | [Web dashboard](14-web-dashboard.md) | Information architecture, pages, flows |
| 15 | [Deployment and operations](15-deployment-operations.md) | podman compose, Helm, configuration reference, upgrades, observability |
| 16 | [Wire protocols](16-wire-protocols.md) | Every inter-component contract and its messages |
| 17 | [Security model](17-security.md) | Threat model and the numbered invariant checklist |
| 18 | [Testing](18-testing.md) | Test pyramid, conformance suites, end-to-end suites |
| 19 | [Implementation plan](19-implementation-plan.md) | Phases, milestones, acceptance criteria |
| 20 | [Mattermost connector](20-mattermost-connector.md) | Phase 1 connector specification |
| 21 | [Later phases](21-later-phases.md) | App catalogue, database lane, HashiCorp Vault store, SSH, previews, evals, … |
| 90 | [Decision log](90-decisions.md) | Every deliberate departure from OneCLI and the proposals, with rationale |

## Phases at a glance

| Phase | Scope |
|-------|-------|
| **0 — Core** | Everything needed for a working platform: identity/tenancy, gateway (MITM + open lane), policy and approvals, secrets with the **pluggable secret-store seam** and the **inline** store, LLM keys with **LLM and egress governance**, the **app plugin system** (manifest, validator, signing, snapshot, mechanisms, hook host, SDK, CLI) proven with fixture packages, BYO and hosted agents (runner, supervisor, harness), conversations, memory, skills, schedules, webhooks, watches, agent links, the **neutral channel core and connector contract** proven with a fake connector, dashboard, podman compose and Helm. |
| **1 — Mattermost** | The Mattermost connector (shared mode, then dedicated mode). |
| **2 — App catalogue** | Built-in app packages (implemented separately against the phase 0 plugin system); the gateway database lane for database apps. |
| **3 — External secret stores** | HashiCorp Vault store (managed and reference modes) on the phase 0 seam. |
| **4+ — Later** | SSH front door, port previews, evals, git skill library, groups/SCIM, additional connectors (Slack is deferred). |

See [19](19-implementation-plan.md) for milestones and acceptance criteria.

## Confirmed scope decisions

- The **app plugin system** is core (phase 0) and is proven with fixture
  packages; the **catalogue of real apps** (GitHub, Gmail, …) is phase 2.
- The **channel core and connector contract** are phase 0 (proven by a fake
  connector and a conformance suite); the **Mattermost connector** is phase 1.
- Aurora has **no edition split** (no cloud/community/enterprise). OneCLI
  enterprise features are either included as plain features (roles, OIDC SSO,
  app availability) or deferred (SCIM, groups, budgets, billing, desktop
  takeover). See `90-decisions.md`.
- **LLM providers are a registry of several providers.** Phase 0 implements
  **LiteLLM** (a self-hosted, operator-configured endpoint) first and
  **Anthropic** second; further providers are registry entries. See `08`.
- **Model and egress governance** (`08 §11`): operators and org admins
  control which model accounts may be used (keys must verifiably belong to the
  approved account, e.g. the corporate Anthropic organization or the internal
  LiteLLM), may refuse agent-supplied LLM credentials (`sanctioned_only`), and
  may default-deny all uncredentialed traffic (`credentialed_only`), at
  instance, org and workspace levels that can only tighten.
- Every new workspace gets a **default hosted agent named `Daneel`**
  (handle `daneel`); the name is configurable. See `10`.
- **Design-review decisions (owner, `80`; `90` D-43…D-58).** The owner's verdicts
  on the design review are written into the owning documents as phase-tagged rules
  ("(phase N, RFE-nnn)"). **Phase 1**: connector-asserted e-mail auto-linking is an
  opt-in per integration (RFE-001); the gateway proxy listener is **TLS-only**
  (RFE-002); the open lane has a posture, CONNECT-time rule semantics and a
  `browser` capability (RFE-003); **sign-in is OIDC only**, the IdP enforcing MFA
  (RFE-004); direct conversations are visible only to their user and workspace
  owners (RFE-009); streaming deltas go through a `DeltaBus` seam (RFE-016).
  **Phase 2**: policy `user` identities bind to the turn's speaker (RFE-005);
  freeze for agents, workspaces and orgs (RFE-006); keyring-style signing key and
  internal secret (RFE-007); atomic audit for the security-critical set (RFE-008);
  conversation delete/archive, redaction, retention and export (RFE-010) with
  attachment retention and quota (RFE-015); approval timeouts, stall exemption and
  notifications (RFE-011a); the `approver` role (RFE-012); management-API rate
  limiting (RFE-017); automation reassignment, offboarding view and workspace
  anchors (RFE-021). Where a phase-tagged rule replaces a phase-0 rule, both stay
  readable in the owning document.
