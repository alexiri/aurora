# TEST-015: Connector conformance suite (fake core)

## Goal
`tests/conformance/connector`: a **fake core** (an in-memory implementation of the connector plane `/v1/connector/*`) plus scripted scenarios that run against any connector binary or in-process `Connector`, green against `connectors/fake`.

## Linked requirements
- `docs/13-channels-connectors.md` §7. Scenario list (all required):
  1. register/approve
  2. DM round-trip
  3. channel mention, then thread follow-up
  4. bot added → binding (the binding flow is core behaviour, so here assert the connector emits `bot.added` and renders the `binding.choose` card it is sent)
  5. unlinked user → link flow (`link-start` called, ephemeral/DM posted)
  6. file in and out (`file.fetch` op → `PUT /files/{op_id}`; `message.post` with files → `GET /files/{id}`)
  7. approval card → click → settle (`action.invoked` carries the opaque token unchanged; `message.update` settles)
  8. credential dies → `presence.removed` or `credential_dead`
  9. a redelivered event is processed once (same `event_id` twice → fake core reports a duplicate; the connector acks the platform only after accept)
  10. a re-claimed post is not duplicated (lapsed lease → `maybe_sent: true` → the connector checks its marker and does not post twice)
  11. broadcast mention defused (`@channel`/`@all`/`@here` in text never reach the platform as broadcasts)
  12. capability fallbacks (connector without `buttons`, etc., per §4.5, which is core-side; here assert the connector honours `capabilities` it declared and returns `unsupported` for ops it can't do)
- §4 shapes; §4.2 neutral error codes and `op_marker`; §6 invariants 3 and 4.
- Wire types: `crates/aurora-protocol/src/connector.rs`. SDK: `crates/aurora-connector-sdk`. Fake connector + control API: `connectors/fake` (`/fake/*`: users, spaces, messages, actions, commands, bot added/removed, presence-removed, posted, failures, reset).

## Scope
### In scope
- New workspace member crate `tests/conformance/connector` (package `aurora-conformance-connector`) with:
  - `fake_core`: axum server implementing `register`, `config`, `events`, `ops/claim`, `ops/results`, `credentials`, `link-start`, `files` PUT/GET, `retire`, held in memory. It enqueues scripted ops, records events/results, and can expire a claim lease on demand. Validate every request body with the protocol types' `Validate`.
  - A `ConnectorUnderTest` abstraction: either spawn a binary with env (base URL, token) or run an in-process `Connector` through the SDK runtime. Implement both; the default test target is the in-process fake connector. The binary mode is used by `scripts/conformance.sh <binary>`.
  - A platform driver trait for the scenarios (for the fake connector: its `/fake/*` control API). Document that a real connector's suite needs its own driver.
  - The 12 scenarios above as `#[tokio::test]`s.
- Workspace `Cargo.toml` member line (append only). Add `scripts/conformance.sh`.
- Fix bugs found in `connectors/fake` or `aurora-connector-sdk`, keeping each fix minimal and listing it in the report. Do not bend the fake core to accept non-conformant behaviour.

### Out of scope
- Real core (aurora-api) and the e2e channel scenario (TEST-016).

## Constraints / file ownership
- You own: `tests/conformance/connector/**`, `scripts/conformance.sh`, root `Cargo.toml` (one member line), and minimal fixes in `connectors/fake/**` and `crates/aurora-connector-sdk/**`.
- Don't touch `services/`.

## Acceptance criteria
- [ ] `cargo test -p aurora-conformance-connector` passes all 12 scenarios against the fake connector.
- [ ] `bash scripts/conformance.sh target/debug/aurora-connector-fake` (binary mode) passes, or the gap is reported.
- [ ] fmt, clippy `-D warnings` clean for the touched crates.

## Dependencies
- Blocked by: CON-001 (done). Independent of BE-070..075. Unblocks the M6 acceptance criterion "connector conformance suite green against the fake connector".

## Suggested agent/model
Sonnet, own worktree.
