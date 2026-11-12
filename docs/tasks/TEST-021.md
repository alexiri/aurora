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
4. **Real front doors in e2e** (`scripts/e2e/gateway.sh` tier or a new
   scenario file): nginx (pinned image) as `aigw`-like front door — `http2`
   on, litellm or the existing mock upstream behind it — with the gateway in
   front; a hosted-harness smoke must get a 2xx through the whole chain. This
   is the production topology regression test. Add haproxy as a second peer
   profile (different strictness quirks, notably h1 keep-alive semantics) if
   it costs < ~20 lines of config.
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
- [ ] e2e nginx front-door scenario green in the serial e2e job
      (`e2e-gateway` CI job); haproxy profile green if added.
- [ ] Non-default-port `Host` synthesis test green.
- [ ] `cargo test -p aurora-gateway --test it` and the e2e suite green; clippy
      `-D warnings`; fmt.
- [ ] `docs/06` rule sentence merged; PROGRESS/FEATURES updated.

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
cargo test -p aurora-gateway --test it proxy::
cargo test -p aurora-e2e-gateway -- --test-threads=1   # incl. nginx scenario
bash scripts/e2e/gateway.sh
cargo clippy -p aurora-gateway --all-targets -- -D warnings
```

## Dependencies

- Blocked by: FIX-052 (its raw-h2 test and load-time guard are the seeds this
  suite generalizes; implement together or TEST-021 immediately after).
- Unblocks: confidence to point the gateway at any strict front door (nginx,
  haproxy, envoy, CERN's `aigw.cern.ch`); a basis for future protocol work
  (e.g. if inbound h2 on the MITM listener is ever wanted, the oracle pattern
  exists).

## Suggested agent/model and reason

rust-developer (Sonnet) — Rust it-tests + one bash e2e scenario + nginx config;
no product code changes expected (if any surface, that is a FIX- spec).

## Completion report

- Files changed: —
- Tests/checks: —
- Remaining risks: —
