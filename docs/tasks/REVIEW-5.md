# REVIEW-5: Independent review of the whole codebase (owner, 2026-11-13)

Owner request: "Review all changes since `09e955c9` … no way to trust what the other model did. Prepare spec files for any changes or fixes … the specs may be wrong too, fix them also if needed", then "Review all the previous commits too".

## How it was done
- **Wave 1** — everything merged after `09e955c9` (phase 4, 243 commits, ~100k added lines), read at `83615626`: thirteen reviewers (SCIM and identity; groups and authorization; grants and parameter matching; counters and gateway wire; previews and relay; streams and terminal; sandbox lifecycle and Codex; skill libraries; placement, config and migrations; two dashboard reviewers; ops; an evidence sweep).
- **Wave 2** — everything built before that (phases 0–3), read at master on 2026-11-13: twenty reviewers (auth and tenancy; secrets and connect; policy, approvals and freeze; gateway transport; gateway decision pipeline; plugin system; built-in packages and CLI; conversations and dispatch; agent tools; channels; connectors; supervisor; runner; database; limits and configuration; two dashboard reviewers; deployment; SEC-01..61; contracts and inventory).
- Every reviewer worked read-only, with no builds or tests (three implementation lanes were running on the machine). Findings therefore rest on reading code, specifications and library sources. Each finding carries `path:line` evidence, a failure scenario and a confidence mark.
- The coordinator re-read the code for the high-severity findings before writing a brief. Where a finding's key step could not be confirmed without running something, the brief says so and requires the failing test first (`LANE_RULES`: a brief that turns out wrong gets an `## Analysis` section, not a "fix").
- Preview, relay and stream findings were re-checked after FIX-072/073 merged (`c3ec906d`); briefs FIX-088..FIX-092 describe what remained and what those fixes introduced.

## Result
About 360 findings, some reported by two reviewers: none critical, about 40 high, about 130 medium, the rest low. They are grouped into forty briefs, `FIX-080` … `FIX-119`, each with disjoint file ownership where lanes could run in parallel and explicit ordering where they cannot.

| Brief | Area | Highest |
|---|---|---|
| FIX-080 | Membership admission gates (SCIM removal vs role claims, placement, invitations); SCIM vs human suspension | high |
| FIX-081 | SCIM protocol and management defects | medium |
| FIX-082 | SCIM adoption of existing accounts by asserted e-mail (owner decision) | high |
| FIX-083 | Group-derived access removal and reporting | high |
| FIX-084 | Approvals re-checked after a hold; missing app fails closed | high |
| FIX-085 | Parameter matching doubt rules; `custom_only` ⇒ `block_untooled`; linter | high |
| FIX-086 | Gateway upgrades only WebSocket; bounded refusal logging | medium |
| FIX-087 | Shared counters (SCIM limiter refund, breaker per class, Redis) | high |
| FIX-088 | Relay HTTP correctness (cookies, stalled pump, timeouts, framing) | high |
| FIX-089 | Preview wake and start flow | high |
| FIX-090 | Relay plane rate class, client address, runner endpoint, token rotation | high |
| FIX-091 | Supervisor stream and PTY pumps; violation window; tests | high |
| FIX-092 | Runner stream listener and terminal API bounds | high |
| FIX-093 | Sandbox start lifecycle (sweep races, unbounded starts, reasons) | medium |
| FIX-094 | Runner robustness (bootstrap claim, listener bounds, k8s errors, DNS check) | high |
| FIX-095 | Harness adapters (Codex config, session refs, stream bounds) | medium |
| FIX-096 | Skill libraries (frontmatter parsing, approval validation, refresh races) | high |
| FIX-097 | Deletion completeness; sign-out ends preview access | high |
| FIX-098 | Terminal tab (colours, CSP, close codes) | high |
| FIX-099 | Dashboard: agent page, chat, onboarding | medium |
| FIX-100 | Dashboard: Directory sync, groups, skill library | high |
| FIX-101 | CI runs what the documents say; skipped suites fail | high |
| FIX-102 | Helm chart (Gateway API preview route, ops ports, probes, hooks) | high |
| FIX-103 | Images and developer tooling | medium |
| FIX-104 | Credential injection only over TLS to the claimed port; wildcard claims; echoes | high |
| FIX-105 | Governed LLM endpoints: inference paths only; key verification (owner decision) | high |
| FIX-106 | Gateway transport hardening; database lane; gateway role | medium |
| FIX-107 | Plugin trust (publisher, revocation, token endpoints, downgrades); hook host | high |
| FIX-108 | Sign-in and sessions (JIT re-admission, re-auth, revocation latency, CLI approval) | high |
| FIX-109 | Freeze authority, approval deadlines, policy matching | high |
| FIX-110 | Agent tools (active turn, link consent, caps, schedule/webhook integrity) | high |
| FIX-111 | Retention jobs; history cursors | medium |
| FIX-112 | Channels (link confirmation, throttles) | high |
| FIX-113 | Connectors and Mattermost | medium |
| FIX-114 | Supervisor (child reaping, bounds, transport wording) | medium |
| FIX-115 | Conversations and credentials (answer buffer, home sync bound, CAS) | medium |
| FIX-116 | Configuration and limits hardening | medium |
| FIX-117 | Dashboard pagination; visual-identity conformance | medium |
| FIX-118 | Built-in packages and CLI | high |
| FIX-119 | Documentation truthfulness; OpenAPI completeness | medium |

Patterns that recur across areas, worth keeping in mind when reviewing the fixes:
1. **Specified, parsed, never enforced** — limits, config keys, retention jobs, notices (the preview limits of FIX-072 were the first; FIX-111 and FIX-116 list some thirty more).
2. **"Proven" by a test that does not run or cannot fail** — env-gated suites that skip in CI, a permission matrix that passes inconclusive rows, static scans standing in for behaviour (FIX-101).
3. **Check-then-act outside a lock**, and full-row writes from a stale read (FIX-080, 081, 093, 096, 110, 115).
4. **A second path that skips the gate the first path has** — role-claim sync vs JIT, placement vs invitations, held approvals vs fresh decisions, plugin upgrades vs first installs, agent-link policy vs approvals (FIX-080, 084, 107, 110).
5. **Flow-control pumps that await a peer** (FIX-088, 091, 092).
6. **Specifications that contradict each other**, where the code followed one side (FIX-082, 098, 105, 108, 110).

## Specification changes made with this review
- `docs/04` §IdP-linked groups (Sync) and C-462: IdP-linked rows are deleted when the org membership leaves `active` (FIX-083).
- `docs/15` §Shared rate-limit counters: the complete Redis ACL (FIX-087).
- Every other specification change is written out in the brief that implements it ("Spec change (apply with the fix)"), so text and code land together.

## Questions for the owner
`docs/PROGRESS.md` §Decisions needed from the owner, Q-R5-1 … Q-R5-23. Briefs that depend on one say so; everything else can start.

## Findings rejected by the coordinator
- **RoleBinding subject without a namespace** (`sandboxes.yaml`, runner Services binding): a namespaced RoleBinding's ServiceAccount subject defaults to the binding's namespace, and the kind e2e passes. Kept as hygiene in FIX-102.
- **Mattermost interactive callbacks forgeable by channel members**: the report assumed the action's integration context is visible to members. Mattermost strips it from posts sent to clients and makes the callback itself, so the forger would need Mattermost administrator or database access. FIX-113 states the trust and adds a conformance assertion instead.
- **Skill-library YAML "billion laughs"**: the parser does limit alias expansion (100 jumps per event), so the exponential form is refused. A product-style document still expands far beyond memory within that limit, so the finding stands in a different form (FIX-096, failing test first).

## What was not covered
Reviewers listed what they did not reach; the main gaps are:
- Nothing was executed. Timing-dependent findings (pump stalls, races) and third-party behaviour (AWS query parsing, Mattermost, podman DNS forwarding, hyper body-drop timing) are reasoned, not observed.
- Not read in depth: `services/aurora-api/src/domains/runners_admin`, the approval notification fan-out, `channels/messaging.rs` internals, mention anchors, the Mattermost `presence.rs`, `aurora-registry` snapshot compilation and claim-conflict resolution, the gateway's `mechanisms/**` (SigV4 finalisation, templates), the hook-host side of the connect protocol, export and retention internals of conversations, most e2e script bodies, most operator pages, and visual checks at 390 px in a browser.
- SEC-01..61: the ledger confirms that every cited test exists; only about a quarter of the invariants were traced clause by clause.
- `docs/90` decisions before D-119 were searched, not read as a set; `docs/MASTER_SPEC.md` was not compared with `docs/00..21`.
- Phase 5 items are not built and were not reviewed (`alexiri/p5-asrc`, `alexiri/p5-guide`).

The briefs cite findings by id (`SCIM-1`, `GWD-4`, …). The coordinator's condensed record of each reviewer's report — every finding with its `path:line`, plus what each reviewer checked and found sound and what it did not reach — is in `docs/audit/review-5/`.
