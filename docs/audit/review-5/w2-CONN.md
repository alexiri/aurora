# REVIEW-5 wave 2 — CONN (connectors, connector host, Mattermost) — saved by coordinator

## CONN-1 (reported high; COORDINATOR: likely NOT exploitable — downgrade to low/verify) — Mattermost interactive callbacks take the clicker from the unsigned `user_id` body field; HMAC covers the token only
- `connectors/mattermost/src/callbacks.rs:66-98` (clicker = `req.user_id` :92); `render.rs:214-232`, `:335-343` (token, sig, iid in the attachment's integration context); core `channels/actions.rs:172`, `:184`.
- Reviewer's premise (context readable by every channel member) is unverified. Mattermost strips action integrations (URL + context) from posts sent to clients (`Post.StripActionIntegrations` in PreparePostForClient) and the Mattermost *server* makes the callback with the clicking user's id. So only someone with Mattermost system-admin/DB access (or network position between MM and the connector) can forge. Action: add a conformance assertion against the real Mattermost (nightly mattermost-conformance) that a non-admin member's fetch of the post carries no integration context; document the trust in the Mattermost server in docs/20; tests for refusal paths (CONN-7).

## CONN-3 (medium, spec-defect by design) — the single shared `auh_` host token authenticates as any managed connector and reads its decrypted integration credentials across orgs; not bound to announced types
- `services/aurora-api/src/domains/connectors/auth.rs:50-64`; `plane.rs:232-259`; `host.rs:155-206`. C-456 ("not an authorization boundary"), C-458, SEC-69.
- Fix: per-host tokens (host row with token hash) + connector type must be among that host's announced types; document blast radius.

## CONN-2 (medium) — Mattermost WS events processed in unordered spawned tasks: delete/edit can reach the core before the post (`connectors/mattermost/src/integ.rs:876`, `:436-487`, `:488-513`)
## CONN-5 (medium, PLAUSIBLE) — tenant-supplied `server_url`: no egress guard in the connector (relies on NetworkPolicy); target's JSON `message` echoed in validate errors (`connectors/mattermost/src/api.rs:81-98`, `map_status`; `connector.rs:230-262`; `values.yaml:417-419`)
## CONN-7 (medium, test-gap) — callback refusal paths untested; conformance suite is happy-path only (`callbacks.rs:117-119`; `render.rs:408-417`; `tests/conformance/connector/tests/scenarios.rs` s01-s14)
## CONN-4 (low, PLAUSIBLE) — slash-command/callback tokens checked against the first integration in the team only (`callbacks.rs:120-122`, `:142`; `connector.rs:357-380`)
## CONN-6 (low) — WS/REST TLS native roots only; `tls.extra_ca_bundle` documented for connectors (docs/15:184) but not consumed (`ws.rs:46-58`; `api.rs:81-98`)
## CONN-8 (low, PLAUSIBLE) — connector plane lease not enforced locally; `lease_secs` never read by the SDK (`crates/aurora-connector-sdk/src/runtime.rs:149-170`; `plane.rs:282`)
## CONN-9 (low) — `plugins.allow_unsigned` has no production guard; D-142 wording mismatch (`crates/aurora-config/src/raw.rs:785-792`; `connectors/plane.rs:100-110`; `host.rs:103`)
## CONN-10 (low) — bootstrap `aun_` token can attach to a managed connector id (`repo/connectors.rs:178-196`; `connectors/auth.rs:80-86`); C-454
## CONN-11 (low) — reaction events skip the team-scope check (`integ.rs:606-640`)
## CONN-12 (low, PLAUSIBLE) — WS auth-failure mapping treats a reply without a code as a revoked token (`ws.rs` seq_reply; `integ.rs:887-890`)
## CONN-13 (low, PLAUSIBLE) — catch-up reads channel posts with `since`, no paging (`integ.rs` catch_up)
## CONN-14 (low) — managed connector in `pending_approval` still receives decrypted secrets in assignments (`connectors/host.rs:232-234`; C-456)

## Not covered: SDK client/sink; Mattermost presence.rs; render mention escaping; migrations for managed connectors; fake/builtin signing; conformance bodies.
