# P2-FU-2: Held requests on a tunnel closed by a BYO tightening or a freeze get a clean 403 (phase 2)

## Problem
`06 §BYO admission` item 1 and C-207 say that when a posture tightening (`byo_disabled`) or a freeze (`frozen`) invalidates a credential, the gateway closes that credential's live tunnels and WebSockets, **and requests held for approval are answered 403 `byo_disabled` / `frozen` with their approvals `abandoned`**. Today the tunnel close drops a held request with a connection reset (FU-P2-025a, found in P2-025). The approval row may stay `pending` until it expires.

## Scope
1. When an `aurora_gateway_invalidate` (scopes `agent`, `workspace`, `org`) makes a cached identity frozen or BYO-disabled:
   - every request of the affected credentials that is **held for approval** (`hold.rs`) is answered with the refusal the admission check would give a new request: 403 `frozen` (`details.level`) or 403 `byo_disabled` (`details.level`), in the gateway's error envelope;
   - its approval is set to `abandoned` (with `resolved_at`, as the existing abandon path does);
   - **then** the tunnel or WebSocket closes. The order matters: the client must receive the 403 before the close.
2. Coverage: requests on both lanes (MITM HTTP inside a CONNECT tunnel, and absolute-form) and every credential kind the rule applies to:
   - `frozen`: `aug_`, `aub_`, `auc_`;
   - `byo_disabled`: `aug_` and BYO `auc_`.
3. Freeze of a hosted agent's sandbox credentials follows the same path (C-207 lists every kind).
4. Metrics: count these answers in the existing refusal counters (`aurora_gateway_frozen_refused_total` and the BYO one), not new metrics.
5. Docs: one sentence each in `06 §BYO admission` 1 and C-207 confirming the order (answer, abandon, close). Decision **D-97** if you make a choice the spec leaves open.

## Constraints
- Don't weaken anything: a held request must never be **released** upstream by this path; it is refused. Audit and request-log rows for these requests use the existing outcome for abandoned approvals.
- No migrations.
- **Owns:**
  - gateway `src/hold.rs`, `src/byo.rs`, `src/proxy/**` (only the close and answer path), the invalidation listener, gateway tests;
  - the two doc sentences above;
  - `docs/API_CONTRACTS.md` only if a contract text changes (no new numbers expected).

## Acceptance criteria
- [ ] Gateway integration tests (in `services/aurora-gateway/tests/it/`): for each credential kind above, a request held for approval when the freeze or posture flip arrives gets 403 with the right code and `details.level`; its approval is `abandoned` with `resolved_at` set; the tunnel is closed afterwards; nothing reaches the upstream mock.
- [ ] Existing hold/approval/freeze/BYO tests stay green; `scripts/e2e/gateway.sh` is green.
- [ ] fmt, clippy `-D warnings` on aurora-gateway, `cargo test -p aurora-gateway`, `bash scripts/guards/run-all.sh`.
