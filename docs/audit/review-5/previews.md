# REVIEW-5 findings — previews (saved by the coordinator from the reviewer's hand-back)

## PRV-1 (high, CONFIRMED relay side) — chunked upstream responses forwarded with their chunk framing inside a second chunked encoding; never end
- `services/aurora-relay/src/proxy.rs:101-115`, `:138-159`, `:328-333` (`is_chunked` unused outside its test); `edge.rs:119-132` (TE dropped at :126). Test `/chunked` (`tests/edge.rs:290`) is a `Full` body with Content-Length.
- Fix: de-chunk leg bytes when the app head is chunked (end at zero chunk; 502 on malformed).

## PRV-2 (high) — proxied HTTP requests leave their stream open after the response (no `Connection: close` to the app; body task waits for app EOF); idle 600 s instead of 120 s → 64-per-sandbox cap exhausted by a page with many subresources
- `proxy.rs:43-45`, `:270-293`, `:347-380`, `:150-154`; `edge.rs:94-116`; `stream.rs:28`; runner `stream_listener.rs:61`; supervisor `streams.rs:457-520`, `:319`. Tests send `connection: close` from the browser (`tests/edge.rs:423`).
- Spec: docs/16:447-449; §W18 "one short-lived stream per request/response".

## PRV-3 (high) — relay-plane calls with the `aul_` bearer are rate-limited as *rejected bearers* (30/min per relay IP); the dedicated preview class is dead code → anonymous cookie spray (or one busy page) breaks every preview
- `services/aurora-api/src/http/apilimit.rs:572-605`, `:624-631`, `:39`, `:241`, `:650`, `:676-690`; `http/auth.rs:467-472`; relay `edge.rs:390-394`, `:406-414`; `api.rs:235-241`. Test `apilimit.rs:848-856` only checks constants.
- Spec: docs/16 §W17 (own per-replica class PREVIEW_API_RATE 100/s burst 200); SEC-85.

## PRV-4 (high, = EVID-1 + EVID-2) — wake flow absent in the relay (no 45 s hold, no 503 refresh page, no try-later page, no 5 s listen wait); positive cache keeps 502 for 15 s; paused/frozen deny reasons answered `not_found` to session holders
- `edge.rs:407-414`, `:614-621`; `state.rs:43`, `:112`; `api.rs:76-82`; `main.rs:103`; api `relay/service.rs:318-327`, `:585-587`, `:307-313`. docs/11:438-453; docs/16 `POST /resolve` row + deny reasons.

## PRV-5 (medium) — whole `Cookie` header removed; the sandboxed app never receives its own cookies
- `edge.rs:103-105`; unit test `edge.rs:1025-1043`; `tests/edge.rs:486`. docs/04:917-923 (remove only the preview cookie); SEC-81.

## PRV-6 (medium, concurrency/revocation latency) — a stalled peer blocks the relay stream pump; revocation / idle / lifetime not processed
- `stream.rs:284-337`, `:289`, `:305`, `:332`, `:321-330`, `:187-206`; `proxy.rs:150-154`. SEC-81 "≤ 15 s for open streams".

## PRV-7 (medium, unwired; not in FIX-072) — idle 120 s, response-head 60 s, chunked request-body cap not enforced
- `limits.rs:295` (`PREVIEW_IDLE_TIMEOUT` unused); `proxy.rs:251-268` (`read_head` no timeout; comment claims 60 s); `edge.rs:903-917` (body max checks Content-Length only); `proxy.rs:72-81`. docs/16:446-449; docs/02:324.

## PRV-8 (medium, test-gap) — relay/preview tests cannot fail on hostile cases (see PRV-1..7 test notes)

## PRV-9 (low, spec) — relay tokens `aul_` cannot be revoked or rotated (`relay/service.rs:157-163`; `repo/preview_relay.rs:189-215`; migration …04; docs/16:356-363; docs/15:921-926)
## PRV-10 (low) — `validate` treats a DB error as "revoked" → closes every open preview stream (`relay/service.rs:658-668`; relay `state.rs:178-188`)
## PRV-11 (low, spec) — header hygiene incomplete/undefined: `clear-site-data`, `alt-svc`, `service-worker-allowed` pass; app CSP dropped; `x-real-ip`, `true-client-ip`, `via`… not stripped (`edge.rs:119-132`, `:94-116`)
## PRV-12 (low) — `authorize` doesn't check freeze/pause → 502 "grant service unavailable" instead of 423/404 pages (`relay/service.rs:414-475`, `:481-511`; `edge.rs:565-568`)
## PRV-13 (low, PLAUSIBLE) — relay dials any endpoint the API names with a live `aux_` grant; `advertised_host` unvalidated (`runner_plane/preview_streams.rs:41-49`; `relay/service.rs:199-257`, `:571-627`; `stream.rs:213-223`)

## Not covered
- `repo/preview_shares.rs` CRUD/record_wake; `previews/service.rs` create/patch/delete/start; hosted wake/start arm; seal/open primitives; api previews.rs/preview_auth.rs line by line.

# Follow-up at HEAD c3ec906d (after FIX-072/073)
- PRV-1 FIXED (ChunkedDecoder, proxy.rs:144) — residual PRV-16.
- PRV-2 MOSTLY FIXED (plain idle 120 s; RelayBody drop detaches the leg proxy.rs:175-177). Remaining: no `connection: close` on the outbound head (proxy.rs `build_request_head`; edge.rs:1002-1020).
- PRV-3 UNCHANGED (apilimit.rs untouched: 572-605, 39, 241, 624-631; relay branch ~680 unreachable for aul_).
- PRV-4 PARTLY FIXED (relay `not_ready` edge.rs ~203 long-polls wait_secs=45, 503 refresh page, `limited` → 429 page edge.rs ~216-230). API still answers allow + start "limited" (service.rs:326-330). Not done: 5 s listen wait (running but not listening → immediate 502, edge.rs ~610-640).
- PRV-5 UNCHANGED ("cookie" in DROP_REQUEST edge.rs ~105).
- PRV-6 UNCHANGED (stream.rs run_leg awaits sink.send / from_app_tx.send inside select; final sink.send(Close) unbounded → slot held forever by a stalled runner). Revocation test tests/behaviour.rs:435-461 is cooperative.
- PRV-7 PARTLY (idle wired; response-head 60 s still unwired proxy.rs read_head; chunked request-body cap edge.rs ~1131 Content-Length only).
- PRV-8 partly; PRV-9, 10 (service.rs:664-676), 11, 12, 13 UNCHANGED.

## PRV-14 (high; code path confirmed) — each 500 ms long-poll iteration burns a wake demand; 12/h budget gone in ~6 s
- api `relay/service.rs` `resolve()` loop ~262-282 → `resolve_once` every 500 ms → `demand_wake` when stopped (~322-324) → `record_wake(…, 3600.0)` (~375; `repo/preview_shares.rs:345-373`); `route_of` maps parked → stopped (:232-236).
- Fix: count the demand once per long-poll / skip while preview_wake_at within PREVIEW_WAKE_WINDOW.

## PRV-15 (high, CONFIRMED) — grant-rate limit keys on the edge's socket address (ingress pod); X-Forwarded-For never read → one 30/min bucket per replica for every visitor, exhaustible anonymously before any cookie/grant check
- relay `edge.rs:1082` (`tcp.peer_addr()`), `aurora_path` ~649-664, `limits.rs GrantRate::check`. docs/15 ~861 says the relay "trusts the edge's overwritten X-Forwarded-For exactly like the API". docs/16 "30/min per client address".
- Fix: `relay.trusted_proxies` mirroring `server.trusted_proxies`; rightmost untrusted XFF entry.

## PRV-16 (medium) — malformed/truncated chunked answers end the body cleanly (browser sees a complete but truncated page) — proxy.rs body task ~146-160, ~180-186. Fix: Err on body_tx so hyper aborts.
## PRV-17 (low, unwired) — "start command exited" page unreachable (`start: "exited"` never set, service.rs:326-330; dto.rs:74; edge.rs ~213-222); "started too often" reports the wake budget, not PREVIEW_START_ATTEMPTS
## PRV-18 (low, spec) — quota and per-share/per-user caps are per replica; `local` fallback gives N × 5 GiB (api `relay/quota.rs:12-19`; relay stream.rs `Streams`); docs/16 says "never fails open" / gives caps without "per replica"
