# REVIEW-5 wave 2 — CHAN (channels plane) and SEC (invariants SEC-01..61) — saved by coordinator

## CHAN-1 (high; mechanism confirmed, needs social engineering) — the sign-in link confirm page never shows the chat account being bound; a chat user can get a victim to bind the attacker's chat identity to the victim's Aurora account
- `services/aurora-api/src/domains/channels/links.rs:168-218` (confirm); `channels/inbox/speaker.rs:95-132`; `web/src/components/channels/link-confirm-view.tsx:84-92`. No endpoint supplies the preview fields.
- docs/13 §5.9 "the page shows the integration, the platform account's handle/display name and the platform email hint if any"; SEC-19.
- Consequence: attacker's chat messages speak as the victim (agent access, approval cards, `approve <code>` — `commands.rs:382-430`).
- Fix: (1) preview endpoint + render on confirm page; (2) stronger: browser-initiated code typed to the bot. Test `channels_mgmt.rs:1534` covers only expiry/cross-user.

## CHAN-2 (medium, unwired) — channel ledgers never pruned (`CHANNEL_LEDGER_RETENTION` `limits.rs:190` referenced nowhere): `repo/channel_inbox.rs`, `channel_ops.rs`, `channel_cards.rs`, `link_requests.rs` have no DELETE. docs/05 §Background jobs "Retention pruning … daily".
## CHAN-3 (medium, unwired) — reach-grant expiry specified (docs/05 "Peer-task and reach expiry — every 60 s") but never happens; nothing sets `expires_at` (`repo/reach_grants.rs:127-140`; `channels/reach.rs`; dead branch `inbox/message.rs:718-727`)
## CHAN-4 (medium) — unlinked speaker gets a new link token + DM for every message; no dedupe/throttle (`inbox/message.rs:136-141`, `:151-165`, `:678-750`; `inbox/speaker.rs:106-117`; `commands.rs:220-232`)
## CHAN-5 (medium, PLAUSIBLE, spec-defect) — approval cards posted to a multi-member channel show method/host/path/body keys to every member (path unmasked) (`channels/cards/build.rs:27-70`; `cards/fanout.rs:118-130`; `crates/aurora-summary/src/lib.rs:120-135`); docs/13 §5.6; SEC-49
## CHAN-6 (low) — `approve <code>` searches only the 200 oldest pending approvals (`channels/commands.rs:~391-420`; `repo/approvals.rs:181-200`)
## CHAN-7 (low) — transient fetch failure re-fetches every attachment → orphans (`inbox/message.rs:262-277`, `:919-943`, `:358-360`)
## CHAN-8 (low) — DM-menu ownership check bypassed once the card's space has been learned (`channels/actions.rs:446-452`, `:557`; `repo/channel_cards.rs:238-252`)
## CHAN-9 (low) — `unbind` writes no audit row (`channels/commands.rs:349-376`)

## CHAN not covered: inbox/mirror.rs, cards/requests.rs, settle logic, deprovision.rs, presences beyond attach, messaging.rs internals, mention anchors, capability.rs/card.rs, e2e.

---

## SEC-1 (high; code gap confirmed, podman DNS forwarding not run) — runner accepts a sandbox network with DNS enabled; SEC-02 "cannot resolve external DNS names" not enforced
- `services/aurora-runner/src/backends/podman.rs:137-165` (`verify_network` reads only `net.internal`), `:177`; `docs/operator/runners.md:81` ("The runner verifies the flag at startup"); docs/11:113-116 (MUST be `--internal --disable-dns`). Scripts always create it correctly so no test sees the DNS-on case.
- Fix: read `dns_enabled` from `/libpod/networks/{name}/json` and refuse unless false (same `dev_allow_external_network` hatch). Test beside `refuses_non_internal_network`.

## SEC-2 (medium, test-gap) — SEC rows resting on e2e suites no CI runs: `scripts/e2e/channels.sh` in no workflow (SEC-18/19/20 cite it; `tests/e2e-hosted/tests/channels.rs`, `mattermost.rs`); `k8s.rs` nightly only (SEC-02, SEC-09); SEC-41 browser path `hosted.rs:911-917` s8d skips unless `AURORA_E2EH_FULL_IMAGE_TAG`; `ci.yml:194-240`; `hosted.sh:92-94`
## SEC-3 (low, docs) — PROGRESS §Open security findings lists a SEC-34 probe that exists (`hosted.rs:762-766`, commit 83befc59; PROGRESS:591, :654, :291) and omits FIX-014
## SEC-4 (low, docs) — `docs/17-security.md:115-124` blank lines break the table (SEC-80..87 render as text); `docs/tests/security-coverage.md:3` header range, `:9-11` duplicated garbled lines
## SEC-5 (low, spec) — SEC-08 ("unknown conditions/mechanisms") and SEC-12 ("every API") too vague; proposed wordings in the report

## Ledger highlights (SEC-01..61): all "enforced: yes" except SEC-02 (partly, DNS) and SEC-08 (wording); tests "partly" for SEC-02, 08, 18, 19, 20 (e2e not in CI), 41 (s8d manual), 29 (cache invalidation not traced). SEC-21/29 depend on `AuthRevoked` LISTEN invalidation (`http/auth.rs:399-420`) — to be confirmed by AUTH reviewer. Most rows "cited" not traced clause by clause.
