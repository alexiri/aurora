# 80 — Design Review (spec-level)

A review of `MASTER_SPEC.md` and `00`–`21`/`90`, looking for design problems,
shortcomings and improvement opportunities **in the specification itself**
(implementation state deliberately ignored). Items that `90-decisions.md`
records as deliberate choices, or that `17 §Residual risks` explicitly accepts,
are not re-litigated here.

**Scheduling** (owner, 2026-11-08): High → phase 1, Medium → phase 2, Low → phase 3, unless the item's Decision says otherwise (RFE-014 → phase 3, RFE-016 → phase 1, RFE-011b → phase 4). Phase 0 is unaffected. Implementation tracking: `docs/PROGRESS.md` §Design-review items; spec changes are phase-tagged "(phase N, RFE-nnn)" in the owning docs.

Each item has: the problem, why it is suboptimal, a high-level fix (to be
specced properly if accepted), and a **Decision** field for the owner's
verdict. Severity is my judgement of impact if left as-is:
**High** = security/privacy hole or a trap for real deployments ·
**Medium** = meaningful gap, workaroundable · **Low** = polish / clarity.


| Id                  | Title                                                                                            | Area         | Severity | Phase               |
| ------------------- | ------------------------------------------------------------------------------------------------ | ------------ | -------- | ------------------- |
| [RFE-001](#rfe-001) | Connector-asserted "verified email" can mint user links                                          | Security     | High     | 1                   |
| [RFE-002](#rfe-002) | Gateway proxy port has no TLS option — BYO tokens travel in cleartext                            | Security     | High     | 1                   |
| [RFE-003](#rfe-003) | Open lane is reachable by any sandbox process and under-specified for path-scoped rules          | Security     | High     | 1                   |
| [RFE-004](#rfe-004) | No MFA for password accounts (including instance admins)                                         | Security     | High     | 1                   |
| [RFE-005](#rfe-005) | Policy `user` identities match "anyone with access", not the actual speaker                      | Policy       | Medium   | 2                   |
| [RFE-006](#rfe-006) | No emergency quarantine / disable switch for an agent, workspace or org                          | Security/Ops | Medium   | 2                   |
| [RFE-007](#rfe-007) | No rotation story for signing key, internal secret, runner session key, gateway CA               | Security/Ops | Medium   | 2                   |
| [RFE-008](#rfe-008) | Audit log is best-effort even for security-critical actions                                      | Security     | Medium   | 2                   |
| [RFE-009](#rfe-009) | Direct (1:1) conversations are readable by every workspace member                                | Privacy      | High     | 1                   |
| [RFE-010](#rfe-010) | No deletion, retention or redaction story for transcripts and attachments                        | Privacy      | Medium   | 2                   |
| [RFE-011](#rfe-011) | Approvals can expire unseen: fixed 300 s deadline, no notification/escalation path               | Product      | Medium   | 2 (011a) · 4 (011b) |
| [RFE-012](#rfe-012) | Role granularity: `can_use` bundles chat, approvals and activity reads                           | Policy       | Medium   | 2                   |
| [RFE-013](#rfe-013) | BYO agent tokens never expire and have no rotation policy                                        | Security     | Low      | 3                   |
| [RFE-014](#rfe-014) | Gateway behaviour during a Postgres outage is unspecified (and the cache flush makes it a cliff) | Ops          | Medium   | 3                   |
| [RFE-015](#rfe-015) | Attachments in Postgres with no retention or per-tenant quota                                    | Ops          | Medium   | 2                   |
| [RFE-016](#rfe-016) | LISTEN/NOTIFY as the delta fan-out has a scalability ceiling with no seam                        | Ops          | Low      | 1                   |
| [RFE-017](#rfe-017) | No general rate limiting on the management API                                                   | Ops          | Medium   | 2                   |
| [RFE-018](#rfe-018) | No idempotency keys for turn-creating API calls                                                  | API          | Low      | 3                   |
| [RFE-019](#rfe-019) | Deployment-specific content baked into the generic spec (Daneel brief, CERN base image)          | Polish       | Low      | 3                   |
| [RFE-020](#rfe-020) | Skills have agent and org tiers but no workspace tier                                            | Product      | Low      | 3                   |
| [RFE-021](#rfe-021) | Creator-bound fire-time authority: offboarding a user silently kills automations                 | Product      | Medium   | 2                   |
| [RFE-022](#rfe-022) | Plugin major-version upgrades with existing connections are undefined                            | Plugins      | Low      | 3                   |
| [RFE-023](#rfe-023) | Default Rule silently exempts LLM hosts — rationale undocumented                                 | Spec clarity | Low      | 3                   |
| [RFE-024](#rfe-024) | Deprovision-before-delete ordering is racy and under-specified                                   | Spec clarity | Low      | 3                   |
| [RFE-025](#rfe-025) | BYO agents: any member can create one, no instance/org/workspace controls, no source restriction | Security     | Medium   | 2                   |


---

## Security and trust boundaries

### RFE-001 — Connector-asserted "verified email" can mint user links

**Severity: High** · Docs: `13 §5.1` step 4, `13 §4.1`, `01 §Trust boundaries` (7), `17` SEC-19/SEC-20

**Problem.** The threat model declares connectors untrusted ("a connector …
cannot act as a user, cannot authorize anything"), and SEC-19 says the core
authorizes every speaker and click from the *user link*. But the user link
itself can be created from data the connector asserts: when a `message.posted`
event carries `email` + `email_verified: true` matching a verified Aurora
user's email, the core auto-creates a `verified_email` link. A malicious or
compromised connector (an adversary the model explicitly lists) can therefore
fabricate an event claiming any platform user has any admin's verified email,
mint the link, and from then on speak and **click approval cards** as that
user. Manifest approval gates the `email_verified` *capability*, but the core
cannot verify which binary holds the `aun_` token, so the capability flag does
not restore the boundary.

**Why it's suboptimal.** It quietly converts "connector compromise" from
"attacker sees one platform's chat traffic" (an accepted residual risk) into
"attacker approves held credential uses and acts as arbitrary users" — a much
bigger blast radius than the documented trust boundary promises, with no
operator knob to prevent it.

**Possible fix (high level).** One or more of:
(a) make verified-email auto-linking an explicit opt-in per integration
(org-admin setting, default off), documented as extending trust to the
connector; (b) always require the sign-in-link confirmation for the *first*
link, allowing the platform email only as a hint that pre-fills the flow;
(c) at minimum, require a one-time user confirmation (dashboard notice "link
your Mattermost account @x?") before the link may authorize approvals/clicks,
even if plain chat is allowed earlier. Update SEC-19/SEC-20 and the residual
risks accordingly.

**Decision:** Implement option a. When auto-linking is disabled for an integration, we get the flow of option b.

---

### RFE-002 — Gateway proxy port has no TLS option; BYO tokens travel in cleartext

**Severity: High** · Docs: `06 §Listeners`, `08 §10`, `15 §Networking`

**Problem.** Port 4102 is a plain-HTTP forward proxy: `Proxy-Authorization`
(carrying `aug_`/`auc_` tokens) and the `CONNECT` preamble cross the network
unencrypted. Inside the cluster/sandbox network that is acceptable; for
**external BYO agents** the spec only says "VPN recommended" while also
specifying LoadBalancer/NodePort exposure of the raw port. A long-lived,
never-expiring agent token (see RFE-013) sent in cleartext over whatever path
a laptop takes to the cluster is a credential-theft channel the rest of the
design works hard to close.

**Why it's suboptimal.** The platform's core promise is credential hygiene;
the front door for BYO undermines it unless every operator independently
builds a VPN. "Secure by default" fails here.

**Possible fix (high level).** Offer a TLS proxy listener (HTTPS proxy): same
semantics, `rustls` with a real certificate (or the external host's cert),
advertised as `https://…` in `gateway.advertised_proxy_url`. Most modern HTTP
stacks support `HTTPS_PROXY=https://…`; document client caveats and keep the
plaintext listener for in-cluster traffic. Alternatively mandate that external
exposure of 4102 is unsupported without TLS or VPN, and make the container
config refuse to emit a plaintext URL for non-local gateways.

**Decision:** Offer a TLS proxy listener only, use it both internally and externally.

---

### RFE-003 — Open lane: reachable by every sandbox process, no per-agent control, path rules unspecified

**Severity: High** · Docs: `06 §Lanes`, `12 §Loopback proxies`, `08 §11.6`, `17 §Residual risks`

**Problem.** Three related gaps:

1. The open-lane loopback proxy (`127.0.0.1:3129`) is intended for the
browser, but any process in the sandbox can use it — and a BYO client can
simply set username `open`. Under the default `open` egress posture this is
an uncredentialed, request-invisible tunnel to anywhere (one log row per
tunnel, host-level policy only). The residual-risk note covers reduced
*visibility*, not the fact that **every agent gets the lane unconditionally**.
2. There is no policy or governance knob to disable or restrict the open lane
per agent/workspace/org ("this workspace has no browser use; no tunnels").
3. The open lane applies "host-level policy", but the semantics of rules whose
targets have `path`/`methods`/conditions at CONNECT time (where only
host:port is known) are unspecified. Does a path-scoped block refuse the
tunnel (doubt tightens) or not apply? Either answer is defensible; the spec
must pick one.

**Possible fix (high level).** (a) Add an open-lane posture (instance/org/
workspace, tighten-only like the rest of governance): `enabled` /
`allowlist_only` / `disabled`; default for sandboxes could be "enabled" but
operators can shut it. (b) Specify CONNECT-time rule evaluation explicitly:
restrictive rules with unmatched-able parts (path/body) treat the part as
matched (consistent with `07 §Conditions and truncation`); allow rules
require full match on what is knowable. (c) Have the supervisor only start
the 3129 listener when the sandbox actually has a browser use-case (or make
it a capability).

**Decision:** Implement options a, b and c.

---

### RFE-004 — No MFA for password accounts

**Severity: High** · Docs: `04 §Authentication`

**Problem.** The spec supports email+password and OIDC, nothing else. There is
no TOTP/WebAuthn second factor and no way to require one. Instance admins —
who approve plugins (arbitrary WASM + egress), connectors, trusted keys and
host claims — can be protected by nothing more than a password, on a product
whose whole purpose is credential custody.

**Why it's suboptimal.** For SSO deployments the IdP covers this, but
`invite_only`/`open` modes and the `bootstrap-admin` path are password-based.
A phished operator password defeats every other control in the system.

**Possible fix (high level).** Phase-tagged MFA: TOTP (and ideally WebAuthn)
enrolment, per-org "require MFA" setting, and a hard requirement (or at least
strong default) that `is_instance_admin` accounts using passwords have MFA.
Alternatively, the minimal version: allow operators to restrict instance-admin
sign-in to OIDC providers only.

**Decision:** All access will go through OIDC only, IdP requires MFA already.

---

### RFE-005 — Policy `user` identities match "anyone with access", not the actual speaker

**Severity: Medium** · Docs: `07 §Rule anatomy`, `06 §Credential authentication`

**Problem.** `principals = {agent_id} ∪ {user ids with can_use on the workspace}` — computed from workspace membership, not from who actually
triggered the turn. A rule "allow tool X when user Alice asks" in fact matches
**every** request by every agent in any workspace Alice can use, whoever is
speaking. The context credential already identifies conversation and turn, so
the actual speaker is knowable, but the evaluator never sees it.

**Why it's suboptimal.** The feature reads like per-requester policy but is
really "workspace roster" policy; administrators will write rules that are far
more permissive than they look. A misunderstanding here grants credentials.

**Possible fix (high level).** Either (a) bind `user` identities to the turn's
speaker when a context credential is present (fall back to never-match or
roster-match — pick one — for boot/agent-token traffic, documented), or
(b) rename/retitle the identity kind in spec and UI to what it means
("workspace member …") and add a separate `speaker` identity later. Fail-closed
preference: (a).

**Decision:** Implement option a, bind `user` to turn's speaker.

---

### RFE-006 — No emergency quarantine for an agent, workspace or org

**Severity: Medium** · Docs: `04`, `05`, `10`, `11`

**Problem.** When an agent is suspected compromised (prompt-injected, leaking,
misbehaving), the operator's options today are: delete the agent (destructive:
home, transcripts' context, presences), regenerate a BYO token, restart a
sandbox, or hand-edit policy to block everything. There is no single
reversible "freeze" that stops turns, revokes gateway credentials, parks the
sandbox and keeps all state for forensics. Orgs and workspaces similarly have
no disabled state (only member suspension, which doesn't stop schedules or
webhooks already owned by others, nor the agent's own background processes).

**Possible fix (high level).** An `agents.disabled_at` (and possibly
workspace/org equivalents): disabled ⇒ gateway refuses its credentials
(existing invalidation machinery), due-work never starts/dispatches, webhooks
answer 410, schedules skip, presences answer "paused". One API call, one
dashboard button, audited, reversible.

**Decision:** Implement freeze.

---

### RFE-007 — No rotation story for the non-keyring key material

**Severity: Medium** · Docs: `08 §3–4`, `06 §Subcommands`, `15 §Backups`

**Problem.** The encryption keyring rotates cleanly (`key_id`s +
`secrets rekey`). Everything else is a single static value with no documented
rotation: `AURORA_SIGNING_KEY` (HKDF root for OAuth/OIDC state, action tokens,
avatar keys), `AURORA_INTERNAL_SECRET`, `AURORA_RUNNER_SESSION_KEY`, and above
all the **gateway CA** (10-year key whose compromise allows MITM of all agent
traffic; rotating it requires re-distributing trust to every sandbox and BYO
host, which nothing in the spec orchestrates). There is also no KMS option for
protecting the inline keyring at rest (Vault in phase 3 covers values, not the
keyring itself).

**Possible fix (high level).** (a) Accept keyring-style lists (old+new) for
the signing key and internal secret so rotation is a rolling config change;
(b) specify a CA rotation procedure: dual-CA period (gateway serves leaves
from the new CA while sandboxes/container-configs trust a bundle of both),
driven by a documented operator runbook; (c) note KMS-wrapped keyring as a
phase-3+ registry option.

**Decision:** implement option a.

---

### RFE-008 — Audit log is best-effort, even for security-critical actions

**Severity: Medium** · Docs: `03 §audit_log`, `04 §Audit`, SEC-32

**Problem.** "Audit writes are best-effort and never fail the action." That is
right for high-volume, low-stakes rows, but it also applies to policy
publishes, grant changes, plugin approvals, trusted-key changes, approval
decisions and role changes. A deployment that cares about SEC-32 cannot
currently guarantee those events are recorded — a DB hiccup (or a clever
attacker timing one) silently loses the trail.

**Possible fix (high level).** Classify audit actions: security-critical
actions write the audit row **in the same transaction** as the change (they
already share the database, so this is cheap and atomic); the best-effort
path remains for everything else. Name the critical set in `04`.

**Decision:** Classify audit actions, log them atomically.

---

### RFE-013 — BYO agent tokens never expire

**Severity: Low** · Docs: `08 §7`

**Problem.** `aug_` tokens have `expires_at = never`; nothing nudges rotation.
Hosted agents got the boot-token treatment (D-14) precisely because long-lived
tokens are a liability; BYO keeps one forever, typically stored in CI configs
and laptops (and sent in cleartext today — RFE-002).

**Possible fix (high level).** Optional per-instance/org maximum age for agent
tokens (expiry visible in the dashboard, regenerate flow already exists), plus
`last_used_at` surfacing of stale tokens. Default could stay "no expiry".

**Decision:** Implement optional per-instance/org maximum age, default to no expiry.

---

### RFE-025 — BYO agents: any member can create one, with no controls over who, where or whether

**Severity: Medium** · Docs: `04 §Permission matrix`, `05 §Agents and sandboxes`, `07 §Injection selection`, `08 §7`, `08 §10`, `08 §11.8`, `06 §Lanes`

**Problem.** A BYO agent is an `aug_` token that works from anywhere that can
reach the gateway proxy port. The spec gives no control over BYO as such:

1. **Who.** Creating an agent needs only `can_use`, so any workspace member
 can create a BYO agent and receive its token. There is no instance, org or
 workspace setting to disable BYO or restrict it to owners (`hosted.enabled`
 controls only hosted agents; `max_agents` is just a count shared with them).
2. **Broad rules arm new agents automatically.** The intended safeguard is
 that a member can create an agent but cannot arm it (grants need
 `can_own`). But injection selection takes *any* enabled allow rule whose
 identities match and whose target names the credential, so a hand-authored
 rule with empty identities (e.g. allow `secrets{scope: organization,  kind: llm}`) selects that credential for every agent, including a BYO agent
 created minutes ago. Owners writing such convenience rules are unlikely to
 realise this.
3. **Uncredentialed egress from the organisation's network.** Even with no
 credentials, a BYO token under the default `open` egress posture turns the
 gateway into a general forward proxy: traffic exits from the organisation's
 network position and IP space, and every destination in
 `gateway.allowed_private` (internal hosts the operator opened for agents)
 becomes reachable from any laptop that holds the token. (The open lane is
 covered by RFE-003; the MITM lane has the same property for plain
 uncredentialed requests.)
4. **Where.** There is no source restriction on token use (no CIDR
 allow-list), so a leaked token is usable from anywhere until someone
 regenerates it (expiry: RFE-013).
5. **Who is actually behind it.** Request-log rows and approvals name the
 agent; the human operating the process is implied only by
 `agents.created_by`, and the token can be shared freely.

**Why it's suboptimal.** Deployments that want "agents run only on our
infrastructure" (hosted only) or "only owners may run agents outside the
platform" cannot express it, and the default lets the least-privileged role
create an externally usable egress identity. The governance layering
(`08 §11`) shows the right pattern exists; BYO just isn't part of it.

**Possible fix (high level).**
(a) A **BYO posture** with the same tighten-only layering as governance
(instance ceiling → org → workspace): `enabled` / `owners_only` / `disabled`.
`disabled` refuses BYO creation and makes the gateway refuse existing `aug_`
and BYO `auc_` credentials of that scope (via the usual invalidation);
`owners_only` requires `can_own` to create a BYO agent or regenerate its
token. A sensible default is `owners_only`.
(b) An optional per-agent **source CIDR allow-list** for BYO credentials,
checked by the gateway at connect time (403 `source_not_permitted`), with an
optional instance/org default list (e.g. "corporate networks and CI runners
only").
(c) A **policy-editor warning** (and an entry in `…/policy/warnings`) when an
allow rule with empty identities selects credentials: "applies to all agents,
including agents created later".
(d) Optionally let a BYO agent be configured to require context credentials
for credentialed requests, so every injected request is attributed to a
labelled run.
Related decisions already taken: TLS-only proxy listener (RFE-002), optional
token maximum age (RFE-013), per-agent open-lane capability (RFE-003).

**Decision:** Implement a (including \``byo.enabled`)`, b, c and d.

---

## Privacy and data lifecycle

### RFE-009 — Direct conversations are readable by every workspace member

**Severity: High** · Docs: `04 §Permission matrix` ("Chat with agents, read transcripts" → `can_use`), `10 §Conversations`, `14`

**Problem.** `direct` conversations are explicitly per-(agent, user) — "one
conversation, several doors" merges a person's dashboard chat and platform
DMs. But transcript reads are gated only by `can_use`, so **every workspace
member can read every other member's 1:1 history with the agent**, including
DMs sent from Mattermost that the sender reasonably believes are private.
Nothing in the spec acknowledges or justifies this.

**Why it's suboptimal.** It is a privacy trap: the platform encourages DM-ing
agents, then silently makes those DMs workspace-public. It also leaks through
side doors (approval cards and activity views name conversations).

**Possible fix (high level).** Decide and document a visibility rule. Options:
(a) `direct` conversations visible only to their user + workspace owners
(owners need repair access); (b) keep them open but say so loudly in spec, UI
and the agent preamble; (c) per-workspace setting. Whatever is chosen, `05`
route permissions and `14` must reflect it.

**Decision:** implement option a.

---

### RFE-010 — No deletion, retention or redaction for transcripts and attachments

**Severity: Medium** · Docs: `03 §6`, `05`, `10`

**Problem.** Memory has revisions **and redaction**; the request log has
retention; the channel ledger has retention. Conversations, transcript events
and attachments have none of it: no `DELETE /v1/conversations/{id}`, no
transcript redaction (secrets pasted into chat live forever), no retention
policy, no export. The only deletion path is deleting the user (directs), the
agent, or the workspace. For a self-hosted product in orgs with data-handling
obligations (GDPR erasure, "don't keep chat logs &gt; N days") this is a gap, and
the `blobs`-in-Postgres backend makes unbounded growth an operational problem
too (see RFE-015).

**Possible fix (high level).** Specify: conversation delete (and/or archive)
with authorization rules; transcript-event redaction mirroring memory
redaction (replace payload in place, audited); optional per-org retention for
conversations/attachments (daily job, same pattern as request-log pruning);
transcript export (JSON) for compliance.

**Decision:** Implement this.

---

## Policy, approvals, product

### RFE-011 — Approvals can expire unseen: fixed deadline, no notification path

**Severity: Medium** · Docs: `06 §Approval holds`, `07 §Approvals`, `02 §Limits` (`APPROVAL_TIMEOUT`)

**Problem.** An approval lives 300 s (one instance-wide config knob). Delivery
is: dashboard SSE (someone must have the tab open), API long-poll (someone
must have built a watcher), or a chat card (only if the conversation has a
thread link). A schedule- or webhook-triggered turn in a workspace with no
channel presence can hit an ask-tool and die silently — nobody is notified
anywhere, and after 5 min the request is refused. There is also no per-rule or
per-tool deadline (a "deploy to prod" ask might deserve hours; chat-based
human response times routinely exceed 5 minutes).

**Possible fix (high level).** (a) Per-rule/per-grant `approval_timeout`
override (bounded by an instance max; the gateway hold already survives long
waits — or specify "park and resume" for long deadlines instead of holding
the connection); (b) a notification seam: email (SMTP already exists) and/or
DM-to-owners when an approval is created and no delivery surface is live;
(c) a visible per-workspace "pending approvals" counter was already specced —
tie it to a browser/web-push notification later.

**Decision:** Split into RFE-011a (stall-exemption fix, modest per-rule timeouts, email-to-owners when no surface is live, and the SSE browser notification) and RFE-011b (Park-and-resume and Web Push). RFE-011b is for phase 4.

---

### RFE-012 — `can_use` bundles chatting, approving and activity-reading

**Severity: Medium** · Docs: `04 §Permission matrix`, `07 §Approvals`

**Problem.** A workspace member can: chat, read all transcripts (RFE-009),
read the request log, **and decide every gateway approval and action
approval**. There is no way to have a member who can chat with agents but not
approve credentialed actions, nor an approver/auditor who isn't a day-to-day
member. For the security posture this product sells, approval authority is
the crown jewel and it is granted implicitly with the cheapest role.

**Possible fix (high level).** Keep the two-level model but add one bit:
workspace access gains a capability set or a third role (e.g. `member` /
`approver` / `owner`, where plain members chat but don't decide approvals).
Alternatively a workspace setting "approvals decided by: members | owners".
Avoid a full RBAC system; one deliberate notch is enough.

**Decision:** Add third role, `approver`.

---

### RFE-021 — Creator-bound fire-time authority makes offboarding destructive

**Severity: Medium** · Docs: `04 §Fire-time authority`, `03 §schedules/webhooks`

**Problem.** Schedules, webhooks and peer tasks re-check their **creator's**
access at every fire and disable themselves (`disabled_reason = authorization`) when it fails. Correct as a security default — but when an
employee leaves and is removed from the org, every automation they ever set up
across the team dies, with only a conversation notice as warning. There is no
ownership transfer, no pre-offboarding report of what will break, and no way
for an owner to re-anchor a schedule other than recreating it.

**Possible fix (high level).** (a) Allow `can_own` users to reassign
`creator_user_id` (audited); (b) an offboarding view: "removing Alice disables
these N schedules/webhooks — reassign?"; (c) optionally allow creating
automations anchored to the workspace (authority = "some owner exists") for
service-like automations.

**Decision:** Implement a, b and c.

---

### RFE-020 — Skills lack a workspace tier

**Severity: Low** · Docs: `10 §Skills`, `03 §7`

**Problem.** Every other shareable resource (secrets, connections, policy,
OAuth clients) is scoped org **or workspace**; skills are agent **or org**
only. A team that wants "all agents in *this workspace* follow our deploy
checklist" must either copy the skill per agent or pollute the whole org.
It's an inconsistency in the scoping model more than a missing feature.

**Possible fix (high level).** Add workspace-tier skills (nullable
`workspace_id` on `skills`, projection precedence agent &gt; workspace &gt; org,
same name-shadowing rule). Cheap now, annoying to retrofit after agents rely
on shadowing semantics.

**Decision:** Add workspace-tier skills.

---

## Operations and resilience

### RFE-014 — Gateway behaviour during a Postgres outage is unspecified

**Severity: Medium** · Docs: `06 §Caches and invalidation`, `01 §The planes`

**Problem.** The hot path is DB-free, but connect-time resolution, credential
auth and approval holds need Postgres. The spec never says what happens when
the DB is down: do cached resolutions keep serving (for how long)? Are new
CONNECTs refused? Worse, `06` says that on LISTEN-connection loss the gateway
**flushes all caches** then reconnects — so the moment the DB blips, the
gateway destroys the only state that could have carried it through, turning a
blip into a full data-plane outage. "Fail closed" is a fine answer, but it
should be a stated decision with the availability consequence acknowledged;
and the flush-on-loss ordering deserves a second look (e.g. flush *after*
successful reconnect, or mark-stale-and-revalidate instead of drop).

**Possible fix (high level).** Specify: (a) resolution-cache behaviour under
DB unavailability (recommend: serve entries within TTL, refuse on miss —
fail closed for *new* state, no cliff for existing); (b) reorder invalidation:
on LISTEN loss, reconnect first, then flush (the window is covered by the TTL);
(c) a metric + readyz distinction so operators can tell "degraded, serving
from cache" from "down".

**Decision:** Implement a, b and c (phase 3).

---

### RFE-015 — Attachments in Postgres without retention or per-tenant quota

**Severity: Medium** · Docs: `10 §Attachments`, `03 §blobs`, `15 §Sizing`

**Problem.** Per-conversation daily caps exist (200 MiB/day out), but nothing
bounds **total** storage per workspace/org or over time; blobs live in a
Postgres `bytea` table forever (S3 is deferred). A busy workspace can grow the
database by gigabytes a week with no pruning job, no quota, no visibility —
`15 §Sizing` discusses only `request_log`.

**Possible fix (high level).** Join this with RFE-010's retention design: a
per-org attachment retention window and/or storage quota (422 on upload when
over), an operator-visible storage breakdown, and a sweep job. Keep the
BlobStore seam as the long-term answer but don't let phase 0 deployments hit
the wall first.

**Decision:** Agreed, implement this.

---

### RFE-016 — NOTIFY-based delta fan-out has a ceiling, with no seam

**Severity: Low** · Docs: `05 §Event bus`, D-03

**Problem.** D-03 is sound for control signals, but `aurora_delta` pushes the
actual streaming text (coalesced to ~2 000 chars/100 ms per turn) through
NOTIFY to **every** API replica. Postgres serializes NOTIFY delivery in the
commit path; with many concurrently streaming agents this becomes a shared
bottleneck well before anything else does. Unlike rate limiting (which got a
`CounterStore` seam) there is no named seam for the delta bus.

**Possible fix (high level).** No behaviour change now; name the seam: deltas
flow through a `DeltaBus` trait whose phase 0 impl is NOTIFY, so a later Redis
pub/sub or direct replica gossip impl is a registry entry, not a refactor. A
rough back-of-envelope ceiling in `15 §Sizing` would also help operators.

**Decision:** Agreed, implement as phase 1.

---

### RFE-017 — No general rate limiting on the management API

**Severity: Medium** · Docs: `04 §Rate limits (auth)`, `05`

**Problem.** Rate limits exist for auth endpoints and webhook deliveries only.
Everything else — message posting, attachment uploads, policy simulation
(which runs the full resolution machinery), memory search, list endpoints —
is unlimited per key/session. Any member (or a leaked workspace key) can
degrade the instance for everyone; there is no per-key or per-org request
budget and no 429 convention outside auth.

**Possible fix (high level).** A modest per-principal token bucket at the API
(in-memory per replica is fine, same approximation as the gateway's), with a
couple of stricter buckets for expensive routes (simulate, uploads, search).
Document it in `05 §Conventions` with the existing 429 + `retry-after` shape.

**Decision:** Agreed, implement this.

---

### RFE-018 — No idempotency keys for turn-creating API calls

**Severity: Low** · Docs: `05 §Conversations and turns`, `10 §Turns`

**Problem.** Channel doors dedupe on `source_event_id`; webhooks on delivery;
but a plain `POST /v1/conversations/{id}/messages` from an SDK retrying a
timeout creates two turns (the second becoming a follow-up). For
"platform-as-API" callers (kind `api` conversations exist precisely for them)
there is no supported way to make message creation idempotent.

**Possible fix (high level).** Accept an optional client idempotency key on
message creation (reusing the `source_event_id` column and its partial unique
index — the mechanism already exists; this is mostly API surface + spec).

**Decision:** Agreeded.

---

## Plugins

### RFE-022 — Plugin major-version upgrades with live connections are undefined

**Severity: Low** · Docs: `09 §10`

**Problem.** "Credential fields may only be added (optional) **within a major
version**" implies a major version may change credential fields incompatibly —
but the lifecycle section never says what happens to existing connections when
a major upgrade activates: do they keep working against a manifest whose
`credentials` vocabulary no longer matches their stored JSON? Are they marked
`reauth_required`? Is the upgrade refused while connections exist (like app
removal)? Templates referencing a renamed field would fail at injection time
with `credential_unavailable`, which is a confusing way to discover a schema
break.

**Possible fix (high level).** Specify: on activation, connections whose
stored credential keys no longer satisfy the new manifest's required fields
are set `reauth_required` with a status reason naming the version change (and
the admin approval screen warns "N connections will need reconnecting").

**Decision:** Agreed.

---

## Polish and spec clarity

### RFE-019 — Deployment-specific content baked into the generic spec

**Severity: Low** · Docs: `10 §Default agent`, `11 §Sandbox image`

**Problem.** Two personal/deployment choices sit in the normative spec:
(a) the **default** `agents.default_brief` is the full Daneel Olivaw persona —
including "deny all connection to all models and AI companies" and a CERN
Building 513 backstory. As a product default, instructing the agent to be
sassy and to actively deny being an AI is a strange (and for some deployments
inappropriate) out-of-box behaviour; it belongs in *this instance's* config,
with the spec default being a short neutral assistant brief. (b) The sandbox
base image is pinned to `gitlab-registry.cern.ch/...` — a private-ish registry
other deployments can't assume.

**Possible fix (high level).** Spec default brief: 2–3 neutral sentences
(concise, ask before irreversible actions, say which tools were used). Daneel
persona moves to the operator's `aurora.toml`. Base image: name a public base
(e.g. AlmaLinux official) with the CERN registry as this deployment's
configured override.

**Decision:** Agreed.

---

### RFE-023 — Default Rule exempts LLM hosts, undocumented why

**Severity: Low** · Docs: `07 §Evaluation` ("block only if facts.injected and not facts.is_llm_host")

**Problem.** The credentialed Default Rule (`default_action = block`) never
applies to governed LLM hosts, so an admin who sets "default: block" and
expects *everything* non-allow-listed to stop will still see LLM traffic flow.
The pseudocode states it; no document explains it (presumably: LLM access is
governance's job and default-deny shouldn't brick the agent's own model loop).
Surprising carve-outs in a security policy engine need stated rationale, and
the policy editor should surface it.

**Possible fix (high level).** One paragraph in `07` explaining the carve-out
and pointing to `08 §11` as the control for LLM traffic; a note in the
dashboard's Default Rule UI ("does not affect LLM endpoints — see
Governance"). Optionally reconsider whether an explicit block rule naming an
LLM host should win (it does today — rules precede defaults — so just say so).

**Decision:** Agreed.

---

### RFE-024 — Deprovision-before-delete ordering is racy

**Severity: Low** · Docs: `04 §Deletion semantics`, `10 §Deleting an agent`, `13 §5.7`

**Problem.** Agent/workspace/org deletion "enqueues connector
`presence.deprovision` operations **before** the delete commits", but the
delete then cascades away the presence (and possibly integration) rows the
connector would need — the op row survives (no FK to presences), yet the spec
doesn't say what the op payload must carry so a connector claiming it *after*
the delete can still act (bot user ref? credentials? the integration may be
gone too for org deletion). Best-effort is fine; unexecutable-by-construction
is not.

**Possible fix (high level).** Specify that deprovision ops are
**self-contained**: payload snapshots the platform_user_ref and the
credentials needed (or states that deprovision is skipped and orphaned bots
are an accepted residual, documented for operators). Also state the ordering
guarantee (ops committed in the same transaction as the delete, before the
cascade).

**Decision:** Agreed.

---

## Not raised (checked and judged fine or already decided)

For the record, things examined and deliberately not listed: per-replica rate
limits (D-05, seam reserved); open-lane reduced visibility per se (accepted
residual — RFE-003 is about *access control*, not visibility); one-conversation-
several-doors merging (D-27); no streaming into chat platforms (later-phase
narration cards); budgets/billing (explicitly deferred); hand-written org
deletion; NOTIFY-as-hints; Postgres-as-queue; the evaluator's injected-only
default semantics; the memory-language design; the sandbox hardening posture
(strong); the testing strategy (unusually good — conformance suites, hostile
fixtures, architecture guards).