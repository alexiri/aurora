# TEST-021: Gateway RFC-conformance suite — strict oracles for the composed wire, real front doors in CI

## Problem

FIX-052 (owner incident 2026-11-12, `aigw.cern.ch`) escaped every gate because
of three structural gaps, not one bug:

1. **Every test oracle was lenient.** The it-suite's mock upstreams are hyper
   servers (`tests/it/proxy.rs`, `auto::Builder`), and hyper *accepts* wire
   shapes nginx rejects (`host` beside `:authority` — RFC 9113 §8.3.1). The
   test TLS even advertises h2 ALPN (`ca.rs:268`), so the bad shape was
   exercised and still passed. A lenient peer proves nothing about compliance.
2. **The production topology was never exercised.** The e2e LLM stack runs
   litellm directly (`scripts/e2e/llm.sh:60` — no front door), while the
   owner's production endpoint is nginx → litellm. The one class of failure
   that only a strict front door produces (bare 400 at the edge) had no
   scenario anywhere.
3. **The composition surface is hand-written but unasserted.**
   `Upstream::send` builds the URI, host header and method itself; nothing
   pinned *which* headers may appear per protocol version, so one obsolete
   insert (a h1-era habit) shipped for months.

## Goal

Make RFC non-compliance **loud and unrepresentable**: pin the exact wire shape
per protocol with strict (non-hyper) oracles, run the gateway against real
strict front doors in CI, and codify the rule the code must live by. Not a
claim of compliance — a harness that fails the moment the wire drifts.

## Environment tiers (VEL-002, owner order 2026-11-12: the local dev loop must not slow down)

Two tiers, by cost — decided by the rule "cheap enough to gate every local
run stays local; everything that starts a container is CI-only":

- **Tier 1 — everywhere (local + CI), in-process, no containers.** The
  raw-h2/raw-h1 strict oracles, exact header sets, framing/SSE/upgrade
  shapes, response stripping and the Host-synthesis assumption test, all in
  `services/aurora-gateway/tests/it/proxy.rs` (single test binary per
  I-34; plain tokio + the `h2` crate already in the tree). These run under
  the ordinary `cargo test -p aurora-gateway --test it` that `just test` /
  `just test-nextest` already execute — **they must stay local**: they are
  the regression gate, and a CI-only regression gate lets the bug come back
  in a local-only change. Budget: the whole addition adds **< 5 s** to a
  local it-run (they are socket + handshake tests on loopback; measured in
  the completion report; if it exceeds, move non-essential cases to tier 2,
  never the FIX-052 core assertions).
- **Tier 2 — CI-only.** The real front doors: pinned nginx and haproxy
  containers in front of the mock upstream, and the hosted-harness smoke
  through the full chain. Implemented in the `tests/e2e-gateway` crate —
  which every local entry point already excludes (`justfile:56`
  `test_excludes`, CI's rust-test/doctests archives, docs/18) and only the
  serial CI job `e2e-gateway` runs. Pattern: `scripts/e2e/gateway.sh` starts
  the front-door containers (pinned `@sha256:` images, like
  `scripts/e2e/llm.sh:25`) and exports `AURORA_E2E_FRONTDOOR`; the new
  `tests/e2e-gateway/tests/frontdoor.rs` skips gracefully when the variable
  is unset — print `SKIPPED: …` and return, exactly like
  `AURORA_E2E_DB_TARGET` (`tests/e2e-gateway/tests/database.rs:53-54`). So
  even a direct local `cargo test -p aurora-e2e-gateway` without the
  environment runs nothing extra. CI job wall-clock budget: +2–3 min (image
  pull dominates); no new workflow job.

Local-loop guarantee (VEL-002): `just test` / `just test-nextest` / `just dev`
wall-clock and CPU are unchanged by TEST-021 except the tier-1 budget above;
no front-door container is ever started outside the e2e script.

## Scope

### In scope

- Gateway upstream-relay wire conformance: MITM lane (POST/GET relay, WS
  upgrade hop), open lane (byte-tunnel — assert transparency, not shapes),
  absolute-form lane.
- Protocol matrix: upstream HTTP/2 (ALPN-negotiated) and HTTP/1.1; framing
  content-length / chunked / SSE-stream / no-body; responses with and without
  hop-by-hop headers.
- Real front doors in the e2e tier: nginx (the class that broke) and, cheap to
  add, haproxy as a second strict peer.

### Out of scope

- The database lane (raw PostgreSQL wire, not HTTP).
- The auxiliary reqwest clients (`mint.rs`, `refresh.rs`, `summarize_client.rs`,
  `governance.rs`) — they compose no headers by hand and rely on reqwest
  defaults (http2 enabled); covered implicitly by the matrix's client-side
  checks only if trivial, else out of scope with a note.
- Fuzzing/property testing (a possible follow-up; this suite is exact-shape).
- Any local-run cost beyond the tier-1 budget above (VEL-002): front-door
  containers, image pulls and the hosted smoke never run outside CI's serial
  e2e job.

## Requirements

1. **Exact-set wire assertions with strict oracles.** For a minimal relayed
   request, a raw `h2::server::handshake` upstream (NOT hyper — that is the
   point) asserts the complete pseudo-header and header set: `:method`,
   `:scheme`, `:authority`, `:path` present; **`host` absent** (RFC 9113
   §8.3.1); **no connection-headers on h2** (§8.2.2: connection, keep-alive,
   proxy-connection, transfer-encoding, upgrade; `te` only with value
   `trailers`); the app's own headers and injected credentials intact.
   Symmetrically, a raw h1 socket-level upstream asserts: `Host` present
   (synthesized by hyper-util), correct origin-form request line, no
   `proxy-*` remnants. Exact sets, not absence-only — an unexpected extra
   header fails the test. (This is the FIX-052 regression test generalized.)
2. **Framing and streaming.** Same strict servers assert: content-length
   preserved on buffered bodies; chunked requests relayed with intact chunk
   boundaries or re-framed equivalently (one of the two, pinned); SSE
   (`text/event-stream`) flows through unbuffered end-to-end; a no-body POST
   (h2 `end_stream` on HEADERS) is sent as end_stream, not a zero-length
   DATA frame with content-length 0 — pick one, assert it.
3. **Response-path conformance.** Upstream responses carrying hop-by-hop
   headers (`connection`, `keep-alive`, `transfer-encoding` name-lists) are
   stripped toward the client on both h1 and h2 client connections;
   `proxy-authenticate` never leaks on non-407 responses.
4. **Real front doors in e2e (tier 2, CI-only)**: `scripts/e2e/gateway.sh`
   gains a front-door stage — nginx (pinned image) with `http2` on in front
   of the existing mock upstream, gateway in front of it — exporting
   `AURORA_E2E_FRONTDOOR=<base-url>`; the new
   `tests/e2e-gateway/tests/frontdoor.rs` runs a hosted-harness smoke that
   must get a 2xx through the whole chain and skips gracefully when the
   variable is unset (never run, never fail, never slow locally). This is
   the production topology regression test. Add haproxy as a second peer
   profile (different strictness quirks, notably h1 keep-alive semantics) if
   it costs < ~20 lines of config. No new CI workflow job — the existing
   serial `e2e-gateway` job runs the script.
5. **The assumption test.** hyper-util's h1 `Host` synthesis (value and
   default-port dropping, `legacy/client.rs` `set_host`) is load-bearing since
   FIX-052. Pin it: a test with a non-default-port upstream asserting
   `host: host:port` reaches the wire — so a future vendored-stack upgrade
   that changes the behavior fails here, not at a customer.
6. **The rule, written down.** `docs/06 §Per-request pipeline` step 13 gains
   one sentence: the gateway adds **no transport headers of its own** to a
   relayed request beyond the declared injections and mechanism outputs; any
   future header the gateway must add (e.g. a new mechanism) lands with an
   exact-set assertion per protocol version in this suite. Reviewers point
   here.

## Acceptance

- [ ] Raw-h2 + raw-h1 strict-oracle tests pass; every assertion is an exact
      expected-set comparison (a deliberately re-added `host` on h2, tried as
      a negative check during development, makes the suite fail — record the
      probe in the completion report).
- [ ] Framing/SSE/upgrade cases green on both upstream protocols.
- [ ] **VEL-002 honored, measured**: a timed `cargo test -p aurora-gateway
      --test it` before/after the tier-1 additions recorded in the completion
      report, delta < 5 s; `just test`/`just test-nextest` start no container
      (grep the script for tier-2 machinery — none reachable without
      `AURORA_E2E_FRONTDOOR`).
- [ ] e2e nginx front-door scenario green in the serial `e2e-gateway` CI job;
      with the variable unset (e.g. plain local `cargo test -p
      aurora-e2e-gateway`) the front-door test prints SKIPPED and passes;
      haproxy profile green if added.
- [ ] Non-default-port `Host` synthesis test green.
- [ ] `cargo test -p aurora-gateway --test it` and the e2e suite green; clippy
      `-D warnings`; fmt.
- [ ] `docs/06` rule sentence merged; `docs/18-testing.md` layers table gains
      the front-door row (CI-only); PROGRESS/FEATURES updated.

## Constraints

- The strict oracles use the `h2` crate already in the dependency tree (via
  hyper) plus std sockets — no new HTTP stacks.
- e2e additions follow the existing conventions: serial, pinned images
  (`@sha256:` like `scripts/e2e/llm.sh:25`), loopback IP budget respected
  (`127.0.0.31-34` in use).
- Never weaken a hygiene behavior to make a test pass; ambiguity between two
  valid behaviors is resolved by picking one and pinning it in the spec text
  (file a FIX- spec if a behavior change is needed).
- Change files only with the Edit/Write tools; never python, sed, awk, perl,
  `cat >>` or heredocs.

## Verification

```sh
# tier 1 (local dev loop — unchanged commands, new assertions included):
cargo test -p aurora-gateway --test it proxy::
cargo clippy -p aurora-gateway --all-targets -- -D warnings

# tier 2 (CI serial job; also runnable deliberately, starts containers):
AURORA_E2E_FRONTDOOR=... bash scripts/e2e/gateway.sh   # via scripts/e2e/gateway.sh
cargo test -p aurora-e2e-gateway -- --test-threads=1 frontdoor::

# local guarantee check (VEL-002): must print SKIPPED, not start containers:
cargo test -p aurora-e2e-gateway --test frontdoor
```

## Dependencies

- Blocked by: FIX-052 (its raw-h2 test and load-time guard are the seeds this
  suite generalizes; implement together or TEST-021 immediately after).
- Unblocks: confidence to point the gateway at any strict front door (nginx,
  haproxy, envoy, CERN's `aigw.cern.ch`); a basis for future protocol work
  (e.g. if inbound h2 on the MITM listener is ever wanted, the oracle pattern
  exists).
- Constraint honored: VEL-002 (owner, 2026-11-12) — no new local-run cost
  beyond the tier-1 < 5 s budget; containerized front doors are CI-only via
  the existing `tests/e2e-gateway` exclusion + env-gating pattern.

## Suggested agent/model and reason

rust-developer (Sonnet) — Rust it-tests + one bash e2e scenario + nginx config;
no product code changes expected (if any surface, that is a FIX- spec).

## Completion report

- Files changed: —
- Tests/checks: —
- Remaining risks: —
