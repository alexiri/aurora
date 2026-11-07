# TEST-016: M6 end-to-end: real core + fake connector

## Goal
Prove M6 acceptance with real processes: the real API/gateway/runner/podman sandbox (fake harness) plus the **fake connector binary** registered through the real connector plane, including the scenario from `docs/19`:
> a fake-platform user DMs the shared bot, gets an answer from Daneel, and approves a held request from a card

## Linked requirements
- `docs/19` M6 acceptance (conformance suite green against the fake connector, which TEST-015 already proved; channel suites of `18`; SEC-18..20; the e2e scenario above).
- `docs/18-testing.md` §Channels. Most items are covered by service tests: `tests/channels_{mgmt,inbox,cards,messaging}.rs`, `connector_ops.rs`, `connectors.rs`. Here cover what needs real processes:
  1. the M6 scenario above end-to-end, where the held request is a gateway approval created by the agent's sandbox request through the real gateway (reuse the TEST-012/013 approval-hold setup) and the card click goes through the fake connector's `/fake/actions`
  2. **ownership failover between two fake connector replicas** (kill one; the other takes the integration over at its next config poll and keeps processing)
  3. verified-email auto-link vs the sign-in link flow, including the `/v1/channel-links/confirm` API call
  4. mirror: an answer posted once, a doorless (schedule) result posted to the last thread link, a web-originated turn not mirrored
  5. a messaging tool (`send_message` with `proactive_messaging: ask`): an action approval card, then approve, then the message appears on the fake platform
  6. `credential_dead` injected via `/fake/failures` disables the presence
- SEC-18..20 in `docs/17-security.md`: read them and make sure each has a proving assertion here or in the service tests. List the mapping in the report.
- Harness: `tests/e2e-hosted` (TEST-013 `Stack`, podman sandbox, `AURORA_TEST_PODMAN=1`), `connectors/fake` binary (`AURORA_FAKE_PORT`, `AURORA_FAKE_CAPABILITIES`, control API `/fake/*`), connector provisioning through the admin API (`POST /v1/admin/connectors`, token, approve with the manifest digest, I-28). If `AURORA_BOOTSTRAP_CONNECTOR_TOKENS` is implemented (FIX-003), you may use it, but don't depend on it.

## Scope
### In scope
- `tests/e2e-hosted/tests/channels.rs` (new) with scenarios 1–6, plus helpers in `tests/e2e-hosted/tests/common/` (append-only, with a fake-connector process manager).
- `scripts/e2e/channels.sh` (or an extension of `hosted.sh`) to run them.
- Minimal fixes to real defects found in core/fake connector, each listed in the report.

### Out of scope
- New features, and UI e2e (Playwright for channels is optional and not required).

## Constraints / file ownership
- You own `tests/e2e-hosted/tests/channels.rs`, `tests/e2e-hosted/tests/common/**` (append-only), `scripts/e2e/channels.sh`, and minimal fixes elsewhere with justification.
- Podman: prefix anything you create with `aurora-e2ec-`, and clean up. Check `df -h /home` before building images.

## Acceptance criteria
- [ ] `AURORA_TEST_PODMAN=1 bash scripts/e2e/channels.sh` passes all six scenarios (paste the summary).
- [ ] SEC-18..20 mapping reported.
- [ ] fmt, clippy `-D warnings` for touched crates; guards pass.

## Dependencies
- Blocked by: BE-070..075 (merged), TEST-013/015 (done).

## Suggested agent/model
Sonnet, own worktree.
