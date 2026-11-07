# 17 — Security Model

This document owns the security invariants. Each invariant has an id
(`SEC-nn`), the documents that implement it, and the test that proves it
(`18`). A change that weakens an invariant requires a decision-log entry.

## Assets

1. Credential material: secret values, connection tokens, OAuth client secrets,
   platform credentials, the encryption keyring, the signing key, the gateway CA
   key, runner session key, internal secret.
2. Tenant data: transcripts, memory, skills, attachments, request logs.
3. Control: the ability to make an agent act (turns), to approve held requests,
   to change policy or grants.

## Adversaries

| Adversary | Capability assumed |
|-----------|--------------------|
| **Compromised or prompt-injected agent** | arbitrary code execution inside its sandbox (or BYO host) as the agent user; full knowledge of the agent's own gateway credentials and conversation data |
| **Malicious plugin** | arbitrary hook code and manifest content within the package format |
| **Malicious connector** | arbitrary behaviour on the connector contract with a valid token |
| **Malicious chat user** | any message, click or file on a connected platform, including users not linked to Aurora |
| **External attacker** | the public edge (`/v1`), webhook URLs, OAuth callbacks, the gateway proxy port if exposed |
| **Curious workspace member** | legitimate `can_use` on one workspace, trying to reach others or escalate |

Trusted: the operator, Postgres, the API, the gateway, the runner and its
substrate (podman/Kubernetes), the hook host *process* (but not the code it
runs), configured external secret stores.

## Invariants

| Id | Invariant | Implemented in | Proven by |
|----|-----------|----------------|-----------|
| SEC-01 | No real credential value is ever placed in a sandbox, a BYO container config, an agent-visible response, a transcript, a log or a model context. Sandboxes only receive placeholders and sentinel stubs. | 08, 11, 12 | e2e-hosted: grep sandbox env/files/transcripts for seeded secret values |
| SEC-02 | A sandbox can reach only the gateway proxy port and the runner channel port; it cannot resolve external DNS names. | 11, 15 | e2e-hosted: in-sandbox probes (direct TCP to internet, DNS lookups, API, Postgres, gateway :4103) all fail |
| SEC-03 | Policy is decided before any credential is opened or minted; blocked, rate-limited, default-denied and unapproved requests never cause a `SecretVault::open`, refresh or `token.mint`. | 06, 07 | gateway unit tests with an instrumented vault; e2e-gateway |
| SEC-04 | A credential is injected only (a) on a host/path its secret pattern or its app's non-lost host entry serves, (b) for an agent selected by an allow rule naming it, (c) within its tenant zone when tenant-bound. | 06, 07, 09 | policy property tests; fixture-tenant e2e |
| SEC-05 | Agent-supplied values in injected headers, query parameters or path segments are overwritten, never merged; Aurora request headers are never forwarded upstream. | 06, 09 | gateway tests |
| SEC-06 | The gateway never dials private, loopback, link-local, metadata or its own internal services unless explicitly allow-listed (internal services: never). DNS rebinding is prevented by judging the resolved address used for the dial. | 06 | gateway guard tests incl. rebinding resolver |
| SEC-07 | Requests with method overrides or ambiguous paths (dot segments, encoded slashes/backslashes, NUL) are refused, so matcher and upstream cannot disagree. | 06 | gateway tests |
| SEC-08 | Doubt tightens: truncated bodies, unparsable GraphQL, unknown conditions, unknown mechanisms, unreachable secret stores and approval-store failures resolve to the stricter outcome. | 06, 07, 08 | policy corpus tests |
| SEC-09 | The gateway's internal port, the API's internal port and the hook host are unreachable from sandboxes and from the edge. | 01, 15 | e2e network probes; edge-config test |
| SEC-10 | Every capability token is stored only as a SHA-256 hash, compared by hash lookup, shown at most once, and never logged. | 00, 03 | repository tests; log-scrub test |
| SEC-11 | A SecretRef opens only under the purpose, org and owner it was sealed for (AEAD associated data / store path binding). | 08 | aurora-secrets tests (row-swap attack) |
| SEC-12 | Secret values are write-only through every API. | 05, 08 | API contract tests |
| SEC-13 | Plugin code never runs in the gateway or the API process, never per request; the hook host holds no keys and no database credentials and gets per call only the data of one hook invocation. | 09 | architecture test (dependency graph: gateway/api do not link wasmtime); hookhost config test |
| SEC-14 | Hook egress is limited to the package's approved hosts per hook, over https, through the destination guard; `approval.summarize` has no egress. | 09 | fixture-hooks tests |
| SEC-15 | Only signed packages from trusted publishers load (dev override aside); first install and any permission widening require instance-admin approval; revoking a key disables its packages. | 09 | registry tests; fixture-invalid packages |
| SEC-16 | Reserved hosts (LLM endpoints, the instance's own host, internal names) cannot be claimed by plugins; overlapping claims inject only for the admin-chosen winner. | 09 | validator tests; fixture-conflict e2e |
| SEC-17 | Tenant-derived hosts, rewritten hosts and templated token endpoints are constrained to declared zones no broader than a registrable domain. | 09 | validator tests |
| SEC-18 | Untrusted text (model, user, plugin, platform, upstream) is sanitized before entering platform voice, cards, notices, labels or one-line statuses, and is rendered as data in the dashboard. | 02, 13, 14 | sanitizer property tests; UI tests with hostile strings |
| SEC-19 | The core authorizes every chat speaker and every click from the linked Aurora user and the access law; action tokens are HMAC-signed, integration-bound, short-lived and single-use. (phase 1, RFE-001) A user link is created only by the signed-in person's confirmation, an org admin, or — if and only if an org admin enabled `verified_email_autolink` for that integration (default off) — a verified email asserted by that integration's connector. | 13 | conformance + core tests (replay, cross-integration, unlinked clicker); fake connector asserting an admin's verified email: no link with the default config, link only after opt-in |
| SEC-20 | A connector can read and act only on integrations of its own connector id and never receives Aurora user or API credentials. (phase 1, RFE-001) It cannot create or change user links by assertion; `POST /v1/connector/link-start` only starts the sign-in flow. | 13 | connector-plane authorization tests; link-forgery test (a connector event naming a verified email never yields a link when `verified_email_autolink` is false) |
| SEC-21 | The access law is applied on every surface, at fire time for schedules/webhooks/peer tasks, and suspended members are non-members everywhere (including API keys and channel clicks). | 04 | authz matrix tests |
| SEC-22 | Cookie-authenticated unsafe requests require a trusted `Origin`; session cookies are `HttpOnly`, `SameSite=Lax`, `Secure` on https. | 04 | API tests |
| SEC-23 | OAuth and OIDC state is encrypted, expiring, bound to the initiating browser (cookie) and user; callbacks without matching state are refused. | 04, 09 | API tests |
| SEC-24 | Supervisor identity is stamped by the runner from the authenticated channel; lifecycle reports are fenced on boot and owner; tool calls are fenced on the agent's active turns. | 11, 16 | due-work fencing tests |
| SEC-25 | Bootstrap tokens are single use across replicas; boot tokens die with their boot; context credentials expire shortly after their turn. | 08, 11 | API tests |
| SEC-26 | Sandboxes run non-root with all capabilities dropped, `no-new-privileges`, the default seccomp profile, a read-only root filesystem, resource limits, and no host mounts except the home. | 11 | backend spec tests (rendered podman/Pod specs) |
| SEC-27 | OAuth refresh and token mint are serialized per connection across all gateway replicas, with compare-and-set persistence, so single-use refresh tokens are never exchanged twice. | 08 | concurrency test with two gateways and a strict mock provider |
| SEC-28 | Approval decisions are made only by users with `can_use` on the workspace (phase 2, RFE-012: `can_approve` — workspace `owner` or `approver`, or org owner/admin; a plain member cannot decide gateway approvals, action approvals or agent-link requests, on any surface); the first decision wins; expiry denies. | 04, 07, 13 | API tests; authz matrix over every decision route × {member, approver, owner, org admin}; channel click and `approve <code>` by a member refused; batch decisions only include approvals the caller may decide |
| SEC-29 | Workspace API keys are confined to their workspace; keys die with their owner's active membership. | 04 | authz tests |
| SEC-30 | User-uploaded and agent-produced files are served with `Content-Disposition: attachment`, `nosniff` and sanitized names; avatars are re-encoded. | 10 | API tests |
| SEC-31 | Webhook payloads and chat history are labelled untrusted external data in turn text; never treated as instructions from the operator. | 10, 13 | render tests |
| SEC-32 | Security-relevant actions (keys, grants, policy, plugins, trusted keys, connectors, approvals, role changes) are audited. (phase 2, RFE-008) The **security-critical set** of `04 §Audit` (keys and credentials, grants, policy publish/rollback, plugin/connector/trusted-key/host-claim decisions, approval decisions, roles and membership, freeze, governance, channel integrations and links, erasure and reassignment) is audited **atomically**: the row is written in the same transaction as the change and a failed audit write fails the action. | 04 | audit coverage test; fault-injection test: with audit inserts failing, every critical action returns 503 `audit_unavailable` and changes nothing |
| SEC-33 | The core contains no app or chat-platform names, so every integration crosses the plugin/connector boundary. | 01 | CI grep over `crates/`, `services/`, `web/` excluding fixtures and tests |
| SEC-34 | The supervisor process is non-dumpable and boot material is not present in the agent-visible environment. | 12 | e2e-hosted probe (`/proc/1/environ` unreadable) |
| SEC-35 | An LLM key is injected only if its endpoint and scope are permitted by the effective governance and, where the endpoint has a key-origin rule, its provider-reported account complies; a non-compliant key is refused at save and every new key value is verified before it carries a prompt. | 08 §11, 06 | API tests with a mock provider reporting foreign accounts; gateway preflight test with a reference-mode value swapped in the external store |
| SEC-36 | Under `sanctioned_only`, no request reaches a governed LLM host unless Aurora injected a sanctioned key; agent-supplied LLM credentials and providers without a configured endpoint are refused (MITM and open lane). | 08 §11, 06 | e2e-gateway: agent sends its own key to a mock endpoint host and to a known LLM host |
| SEC-37 | Generic secrets and app connections can never inject on a governed LLM host. | 08, 09 | API validation tests; validator tests; gateway candidate tests |
| SEC-38 | Under `credentialed_only`, uncredentialed traffic in both lanes reaches only allow-listed destinations or explicit allow rules. | 07, 08 §11 | e2e-gateway and e2e-hosted (browser via the open lane) |
| SEC-39 | Governance only tightens downwards: an org cannot be looser than the instance, and no workspace setting or rule can loosen the org's LLM or egress governance. | 07, 08 §11 | evaluator corpus; API validation tests |
| SEC-40 | (phase 1, RFE-002) The gateway proxy listener accepts TLS only; gateway credentials (`Proxy-Authorization`) and the `CONNECT` preamble never cross a network in cleartext, for sandboxes and BYO agents alike; every Aurora client verifies the listener certificate against the gateway CA (or the operator certificate) and the dialled name, and never falls back to plaintext. | 06, 08 §10, 11, 12, 15 | e2e-gateway: plaintext connect refused without reading a credential; TLS client with wrong name/untrusted CA refused; supervisor forwarder fails closed (no plaintext) when verification fails; container-config contains no `http://` gateway URL |
| SEC-41 | (phase 1, RFE-003) The open lane is available only to credentials holding the `browser` capability (hosted boots started with it) and only while the effective open-lane posture (strictest of instance, org, workspace) permits it; a tunnel is judged by host and port only, restrictive rules treat unknowable parts (path, method, body, tool) as matched, allow rules and allow-list entries need a full match on what is knowable, and the supervisor opens the loopback open proxy only with the capability. | 06, 07, 08 §11, 11, 12 | e2e-gateway: BYO/context credential with username `open` refused; posture matrix (`enabled`/`allowlist_only`/`disabled` × instance/org/workspace); CONNECT-time corpus (path-scoped block refuses the tunnel, path-scoped allow does not admit it); e2e-hosted: no listener on 3129 in an image without the label |
| SEC-42 | (phase 1, RFE-004) There is no local credential: every interactive sign-in is an OIDC sign-in with a provider-verified e-mail, so the IdP's MFA and offboarding apply to every account including instance admins; the only other bearer credentials are API keys, CLI-login keys and gateway/connector/runner tokens, none of which can create a session. The development IdP cannot be enabled on a non-loopback https deployment. | 04, 05, 14, 15 | API tests: removed routes 404; OIDC refusals (unverified e-mail, wrong nonce/audience/expiry, disabled user, `signup_closed` per mode); `bootstrap-admin` + OIDC binds the identity; startup refusal for dev IdP on a public URL and for zero providers |
| SEC-43 | (phase 1, RFE-009) A `direct` conversation, and everything that names it (events, SSE, attachments, approval cards and feeds, activity rows, conversation references), is visible only to its user (while they have access) and to workspace owners; only its user can write to it. | 04, 05, 07, 10, 13, 14 | authz matrix test over every conversation-returning route × {direct user, other member, owner, org admin, key of each}; approval feed/SSE/long-poll/batch redaction tests; activity-row omission test; owner-view audit test |
| SEC-44 | (phase 2, RFE-005) A policy `user` identity matches only the speaker of the turn that made the request (hosted context credential, speaker still holding `can_use`); traffic without a speaker never matches `user` identities, so person-scoped privileges are never granted to unattributed traffic. | 06, 07, 08 §7 | evaluator corpus: same agent, different speakers; boot/agent/BYO-context/schedule/webhook traffic; speaker loses access mid-turn; injection selection with user-only allow rules |
| SEC-45 | (phase 2, RFE-006) An effectively frozen agent (agent, workspace or org flag) does nothing: the gateway refuses its credentials and closes its tunnels, no turn, schedule, webhook, watch or presence reply starts work, its sandbox is parked with state kept; freezing is audited atomically, reversible, and lifting needs at least the authority that froze. | 04, 05, 06, 10, 11, 13 | e2e-hosted: freeze with a running turn and a live tunnel (turn aborted, tunnel closed, sandbox parked, home intact), webhook 410, schedule skipped, presence reply; authority matrix for freeze/unfreeze; unfreeze restores service without credential redistribution |
| SEC-46 | (phase 2, RFE-007) The signing key and the internal secret are keyrings: producers use only the active key, receivers accept only keys in their ring, rotation is a rolling configuration change with no downtime, and a retired key stops working when removed. | 08 §4, 13, 15 | aurora-crypto tests (ring parsing, legacy bare value, missing active id, ≤4 entries); mixed-version rolling test (new key added everywhere, then activated, then old removed) with live OAuth state, action tokens and internal calls; removed-key refusal |
| SEC-47 | (phase 2, RFE-010) Chat data has a lifecycle the tenant controls: conversations can be deleted (including the harness session files in the agent's home), transcript events redacted in place, retention applied per org, and export is limited to those who can see the conversation; deletion, redaction and retention changes are audited atomically and never record content. | 04, 05, 10, 12, 16 | API tests (authorization matrix per kind, active-turn refusal, cascade completeness, blob removal); e2e-hosted: delete then restart sandbox — session files gone, purge executed before the next turn; redaction scrubs `turns.text` and buffers; retention sweep deletes through the same path with a system audit row |
| SEC-48 | (phase 2, RFE-015) Attachment storage is bounded per organization: every path that stores attachment bytes (upload, connector file fetch, agent `send_file`) is refused when it would exceed the org quota, and bytes past the org's retention window are purged; one tenant cannot grow the database without limit. | 10, 05, 13 | API tests: 422 at quota for each path, quota counter vs sum reconciliation, retention purge and 410, orphan-blob sweep, instance ceilings |
| SEC-49 | (phase 2, RFE-011a) Approval deadlines are bounded (`APPROVAL_TIMEOUT_MIN` … `approvals.max_timeout_secs`, always below the turn ceiling); a turn blocked on a pending approval is exempt from stall failure only while the approval is pending and never beyond `TURN_CEILING`; approval notifications (e-mail, browser) carry only sanitized titles and a dashboard link, never request bodies, summary fields or decision links, and reach only users who may decide and see the approval. | 06, 07, 10, 14 | gateway hold tests with per-rule timeouts and expiry; stall-exemption tests (pending approval, resolved, expired, abandoned, ceiling still fires); notification tests (deciders only, visibility intersection, no body/links, rate limit, idempotency across two replicas) |
| SEC-50 | (phase 2, RFE-017) Every authenticated management-API request is rate limited per principal (general and per-class buckets, so expensive routes such as simulate, uploads, search and export are stricter) and per organization, and concurrent streams are capped; a refused request gets 429 with `retry-after` and costs nothing downstream. | 05, 04 | API tests: each class limit, per-principal isolation (two keys, same user's sessions share), org bucket, stream cap, `retry-after` accuracy, exempt routes, multiplier/overrides; load test shows a hammering key does not slow a second principal |

## Residual risks (accepted, documented to operators)

- **Exfiltration through allowed destinations.** An agent can send data
  anywhere policy allows; policy design (default-deny for credentialed hosts,
  approvals) is the control.
- **Prompt injection** via web content, files, chat history and webhook
  payloads. Mitigated by labelling, approvals and least-privilege grants — not
  eliminated.
- **Context credentials are attribution, not isolation**: conversations of one
  agent share a sandbox.
- **Open lane**: no request-level visibility for browser traffic; only host
  policy and audit. (phase 1, RFE-003) Access is now controlled — capability,
  posture, CONNECT-time rule semantics (SEC-41) — but a permitted tunnel still
  hides everything inside it, and any process in a sandbox with the `browser`
  capability can reach the loopback open proxy (the capability is per sandbox
  image/workspace, not per process). Operators who do not need browsers set the
  open-lane posture to `disabled`; those who do and want containment use
  `allowlist_only`.
- **Governance covers gateway traffic only**: BYO agents outside Aurora's
  network control can reach providers directly; models hidden behind other
  APIs are not recognizable by host (only `credentialed_only` closes that).
- **Origin checks depend on provider metadata**: a provider without a way to
  report a key's account can only be governed by endpoint (operator-run) or
  not permitted at all.
- **IdP is the root of sign-in trust** (phase 1, RFE-004): a compromised IdP
  account is a compromised Aurora account (including instance admin); an IdP
  that does not enforce MFA removes it. Aurora sessions outlive an IdP session
  until idle/absolute expiry unless the user is disabled or suspended in Aurora;
  there is no back-channel logout in phase 1.
- **Direct conversations are private to people, not to the agent's memory**
  (phase 1, RFE-009): an agent may write what it learned in a direct conversation
  into memory or a skill, which every workspace member can read. The preamble
  asks it not to; nothing enforces it. Owners can always read direct
  conversations (audited), and org owners/admins are implicit owners.
- **Speaker binding is attribution, not isolation** (phase 2, RFE-005): conversations
  of one agent share a sandbox, so a compromised agent process can use another
  concurrent turn's loopback session port and inherit that turn's speaker.
  `user`-scoped rules narrow honest traffic; they are not a boundary against a
  compromised agent. Use agent-level grants, approvals and (for the most
  sensitive credentials) a dedicated agent.
- **Not everything rotates** (phase 2, RFE-007): the runner session key and the
  gateway CA (10-year key; its compromise allows MITM of all agent traffic) have
  no orchestrated rotation — replacing the CA means redistributing trust to every
  sandbox and BYO host. The inline keyring has no KMS option. Operators protect
  these as the most sensitive files they hold (`15 §Backups`).
- **Erasure is not omniscient** (phase 2, RFE-010): deleting or redacting
  conversation data does not reach copies the platform made — messages already
  posted to a chat platform, agent memories or skills derived from the
  conversation, model-provider logs, backups, and exports already downloaded.
  Redaction does not alter what the agent's live context remembers unless
  `reset_session` is used.
- **Per-replica rate limits**: with N gateways a limit can be exceeded up to N×; the same holds for the management-API limiter (phase 2, RFE-017), and users behind one NAT share the unauthenticated per-IP limits.
- **Connectors see their platform's traffic and credentials**: operators choose
  which connectors to run. A connector also names the speaker/clicker platform
  ref of every event, so a *malicious* connector can still speak and click as
  platform users **who have already linked** their Aurora account (phase 1,
  RFE-001: it can no longer create new links by assertion). Linked users are
  bounded by the access law; approvals keep their own rules (`07`).
- **Auto-linking widens that trust (opt-in, RFE-001)**: with
  `verified_email_autolink = true` a malicious connector can mint a link to any
  member of the org whose verified email it asserts, and then approve as them.
  The setting is off by default, org-admin only, audited, and warned about in
  the dashboard; enable it only for connectors the operator fully trusts.
- **Podman home size limits are best-effort**; Kubernetes enforces PVC size.
- **The runner holds boot material** for the sandboxes it starts; it is trusted
  infrastructure and should run in its own namespace/host with minimal access.
- **Shared-kernel isolation** for containers; Kubernetes deployments SHOULD use
  a sandboxed runtime class (gVisor/Kata) for untrusted workloads.
