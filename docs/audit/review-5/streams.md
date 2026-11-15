# REVIEW-5 findings — streams (saved by the coordinator from the reviewer's hand-back)

## STRM-1 (high, PLAUSIBLE: mechanism confirmed) — supervisor preview pump awaits per-stream queues; one non-reading app stalls every stream + share updates + reset of that sandbox
- `services/aurora-supervisor/src/streams.rs:381` (`on_data` awaits `l.data_tx.send`), `:39` (`DATA_QUEUE = 16`), `:514` (`sock.write_all` inside the recv arm), `:289`, `:241` (credit sends, cap 8 `:343`), `:399-406` (`apply_shares`), `:37-38` comment. Runner sends each WS message as its own frame (`stream_listener.rs:1278-1292`). Runner side fixed by FIX-060 (`stream_listener.rs:349-363`, `DATA_Q = 256`, try_send).
- Consequence: `Cmd::Shares` never applied → removed share keeps streaming (SEC-82); Reset queued behind blocked pump.
- Fix: `try_send` + kill stream with `error` on Full; write in a select arm / write timeout; derive DATA_QUEUE from window. Owns `services/aurora-supervisor/src/streams.rs`.
- Test: loopback listener that never reads; 64 one-byte frames; pump still answers close and share removal within 1 s.

## STRM-2 (medium, PLAUSIBLE) — PTY task awaits outside its select (write loop, reap); one shell not reading stdin wedges the sandbox's PTY pump
- `services/aurora-supervisor/src/pty.rs:636-657`, `:606`, `:613`, `:632`, `:659` (`reap`), `:306`, `:257` (resize cap 4 `:370`), `:249`, `:311`, `:408` (data cap 16 `:34`), `:316-318`, `:430-434`.
- Scenario: paste ~100 KB into `sleep 600`; close tab → `pty.close` never read; new terminal times out (4502).

## STRM-3 (high; counter mechanics CONFIRMED) — supervisor protocol-violation counter never decays; after 4 benign close races every further one aborts the channel (4400), dropping all terminals/streams
- `streams.rs:216` (init), `:388` (inc), `:389` (>= 4), `:409-414` (reset doesn't clear), `:369-373`, `:292-297`, `:417-422`; `channel.rs:531-541`; `pty.rs:225`, `:395-399`, `:414-419`; `main.rs:164` (spawn once per process).
- Spec defect too: docs/16 §W18 (~430) "repeated violations" undefined; data racing a close is legal on the wire.
- Fix: window/per-connection counting; spec: N within a window; data for an id this side just closed is dropped silently.

## STRM-4 (high, test-gap) — supervisor stream engine has no test; SEC-82 claims service-test proof
- `streams.rs:592-605` only `frame_data_is_tag_id_payload`; runner suites use a fake supervisor. docs/17:118.
- Add `services/aurora-supervisor/tests/it/streams.rs`: unlisted port refused `not_shared`; 3128 never dialled; empty list refuses; share removal closes live streams; over-window → error; 65th open refused; `::1` fallback; non-loopback not reached.

## STRM-5 (medium, security; reachability depends on NetworkPolicy) — runner stream listener: unbounded pre-auth connections; header read has no total deadline (slowloris)
- `services/aurora-runner/src/stream_listener.rs:591-606`, `:633-645`.

## STRM-6 (medium, spec-code-divergence) — idle timeout ignores ping/pong (spec: "pings count"); a quiet terminal is closed 4504 after 600 s
- `stream_listener.rs:1298`, `:1104`, comment `:60`; api `terminal/service.rs:522` drops browser pongs. docs/16 ~447, ~489.

## STRM-7 (low) — terminal proxy: browser Text unbounded; browser-side writes have no timeout (stalled tab holds a slot)
- `services/aurora-api/src/domains/terminal/service.rs:509-517`, `:541`, `:580-585`, `:479`.

## STRM-8 (low) — `?wait` on the terminal upgrade ignored; `TERMINAL_WAKE_WAIT` unwired
- `terminal/service.rs:175`, `:146-147`; `routes.rs:14-17`; C-616 (API_CONTRACTS:1041).

## STRM-9 (low, spec) — no close audit row for terminal sessions (C-618 open-only)

## STRM-10 (low) — closing a terminal drops the master without killing the shell's process group (HUP-ignoring shells survive)
- `pty.rs:316-318`, `:430-434`, `:542`, `:466-469`; test `pty_leak.rs:83-109`.

## Not covered
- runner TLS / NetworkPolicy reachability of 4108; schema field-by-field; channel.rs reconnect internals; resize bounds; runner per-sandbox outbound unbounded channel (`channel/mod.rs:216`); bodies of terminal_streams/streams tests.

# Follow-up at HEAD c3ec906d (after FIX-072/073): STRM-1..10 ALL UNCHANGED (pty.rs lines moved: write loop :660, reap :624/:631/:650/:677, resize_tx.send :324, data_tx.send :426, credit/resize :263/:271/:329). Production uses StreamListener::new → with_limits(600 s, 4 h).

## STRM-11 (medium; mechanism confirmed) — `PtyHandle::close_all` (pty.rs:102-107) awaited with no timeout before the shutdown grace (supervisor.rs:348, grace computed :350-351); a wedged PTY pump (STRM-2) hangs shutdown forever. Test `pty_leak.rs shutdown_closes_every_pty` uses healthy PTYs only.
## STRM-12 (high, PLAUSIBLE: mechanism confirmed, growth rate not measured) — runner per-sandbox outbound queue is unbounded
- `services/aurora-runner/src/channel/mod.rs:216` (`mpsc::unbounded_channel()` per W6 connection), `:180-201` (`push_stream_frame`/`push_stream_data` no bound); writer `channel/conn.rs:253-263`.
- Producers: `pty.resize` text from the browser (stream_listener.rs terminal leg ~1101-1110: every text message → PtyResize, no rate limit/coalescing/credit; API forwards text unbounded, STRM-7); legs without a runner-side per-sandbox cap (each can queue a 128 KiB window).
- A supervisor that stops reading → runner memory grows without bound → runner OOM drops every sandbox on the node.
- Fix: bounded queue, close on Full; coalesce resize (latest, ≤ every ~50 ms per leg); per-sandbox live-leg cap in the runner.
## STRM-13 (low) — docs/12:505 "Everything closes on shutdown": `Supervisor::shutdown` (supervisor.rs:341-363) never closes preview streams (relies on process exit). Add `StreamsHandle::close_all` or amend.
