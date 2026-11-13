# REVIEW-5 wave 2 — RUN (runner/sandbox), CONV (conversations/dispatch), WEBX (web chat/agent/channels/admin) — saved by coordinator

## RUN-1 (high, CONFIRMED) — a transient API error after the bootstrap claim leaves the boot permanently unable to connect
- `services/aurora-runner/src/channel/conn.rs:147-150` (claim), `:170-173` (`channel_hold` failure → CLOSE_TRY_LATER 1013 before any welcome); supervisor `channel.rs:291`, `:335-342` (reconnects with the same bootstrap token; 4409 fatal); API `repo/sandboxes.rs:1107` (`bootstrap_used_at` set at claim). Session path has `hold_pending`; bootstrap path doesn't.
- Fix: after a successful claim, a retryable hold failure admits with `hold_pending = true` + welcome; `retry_hold` in background.

## RUN-2 (high, CONFIRMED) — `ApiClient::claim` retries a non-idempotent POST; a committed claim whose response was lost comes back 409 → 4409 → boot dead
- `services/aurora-runner/src/api.rs:231-235`, `:154-170` (retries transport/5xx/408/429 ×5, `:68`); `conn.rs:147-148`. Comment says "No retry on 409/404".
- Fix: single attempt, and/or API claim idempotent for a re-presented token while the boot has no channel hold (`runner_plane/service.rs:242`; `repo/sandboxes.rs:1101`); SEC-25 wording.

## RUN-3 (high, CONFIRMED) — sandbox-facing channel listener (4105): no pre-auth connection bound, no WebSocket handshake timeout → one sandbox can exhaust the runner's fds (cross-tenant)
- `services/aurora-runner/src/channel/mod.rs:470-480`, `:255`, `:463`; `channel/conn.rs:51` (`accept_async_with_config` no timeout; `hello_timeout` only after handshake ~:133).

## RUN-5 (high if confirmed; PLAUSIBLE) — k8s RoleBinding for runner Services: ServiceAccount subject without namespace (probably matches nothing / rejected by apiserver)
- `deploy/helm/aurora/templates/sandboxes.yaml:80-85` (first binding `:65` has namespace); runner `backends/kubernetes/mod.rs:300`, `:427` (`services().get`). NOTE: kind e2e passes 4/4 per PROGRESS — check before believing; apiserver validation requires namespace for ServiceAccount subjects (would fail at install), unless templated otherwise.

## RUN-4 (medium) — runner ops listener (unauth /healthz,/readyz,/metrics) defaults to `server.bind` = 0.0.0.0, sandbox-facing on podman
- `crates/aurora-config/src/validate.rs:871-876`; `services/aurora-runner/src/main.rs:82`, `:70`, `:78-80` (overlap only warns); `crates/aurora-core/src/service.rs:152-165`; `docs/operator/runners.md:50`, `:62`.

## RUN-6 (medium) — final report flush at shutdown drops whole batches on the first transient error (`reporter.rs:114-120`; `runner.rs:285-288`)
## RUN-7 (low) — runner Role grants get/watch on Secrets it doesn't use (`sandboxes.yaml:23-27`)
## RUN-8 (low; = SEC-1) — podman network check doesn't require DNS disabled (`podman.rs:137-164`)
## RUN-9 (low; = CONV-6) — `/runner/sandboxes/check` answers for any sandbox id across runners (`repo/sandboxes.rs:1201-1208`; `runner_plane/service.rs:307-318`)

## RUN not covered: runner.rs poll loop; plane.rs/hub.rs; executor start/stop; workloads.yaml runner SA; live SEC probes mapping.

---

## CONV-1 (medium, CONFIRMED) — a failed/stalled turn never drains its answer buffer: post-tool answer text lost; buffer row persists
- `services/aurora-api/src/domains/hosted/lifecycle.rs:42-54`; `hosted/reports.rs:474` (`answer_buffer_take` only in `result()`); `repo/hosted.rs:1090-1100`. docs/10 §The answer rule.

## CONV-2 (medium, PLAUSIBLE) — `render_inputs` home part never size-checked (peers/channels/accounts/schedules unbounded) → oversize `home.sync` never converges
- `hosted/home.rs:356`, `:317-323`; `crates/aurora-protocol/src/runner.rs:806-815` (MAX_HOME_SYNC_PART 200 KiB).

## CONV-3 (medium, PLAUSIBLE) — `set_session` overwrites `harness_session_ref` without purging the old one → harness session files survive conversation deletion (SEC-47)
- `repo/hosted.rs:331-348`; `repo/lifecycle.rs:73-117`.

## CONV-4 (low) — no-key sweep reaches only the 16 oldest queued hosted turns (`repo/hosted.rs:660-670`; `runner_plane/work.rs:165-200`)
## CONV-5 (low) — boot capabilities include `terminal` even when the runner didn't advertise it (`runner_plane/boot.rs:235-240` vs `service.rs:66`; C-623)
## CONV-6 (low) — `sandboxes/check` existence oracle across runners
## CONV-7 (low, PLAUSIBLE) — peer names, channel tenant labels, schedule names enter instructions unsanitised/unlabelled (`hosted/home.rs:219`, `:115`, `:200`; SEC-18/31)

## CONV not covered: protocol schemas drift; export/retention sweep; tests; per-turn durable event count cap (none found, no spec bound).

---

## WEBX-1 (medium) — Admin Organizations page reads one page, ignores cursor (`web/src/lib/api/freeze.ts:87-92`; `admin/orgs-view.tsx:27-31`; C-203)
## WEBX-2 (medium) — org attention list for agent tokens truncates to newest 100 (drops the oldest = most stale) (`lib/api/agent-tokens.ts:66-74`; `governance/byo-panel.tsx:58`; C-313)
## WEBX-3 (medium) — agent conversation list shows only the first 50; deep link beyond loses metadata (`lib/api/conversations.ts chatApi.list`; `agents/chat-tab.tsx:56,64-70`)
## WEBX-4 (medium) — choosing "Default" effort is never saved (`agents/instructions-tab.tsx:50`, `:125`) → send `effort: null`
## WEBX-5 (medium) — any non-2xx stream response reported as "too many streams"; transcript never reconnects (`lib/chat/use-transcript.ts:38-45`; docs/14:36-38; C-53)
## WEBX-6 (medium) — Retry re-post of a failed turn is fire-and-forget: no Idempotency-Key, no error shown (`chat/message-group.tsx TurnErrorWithRetry onRetried`; RFE-018; C-615)
## Lows: WEBX-7 person attachments render as "Attachment" (message-group.tsx:125-127); WEBX-8 applyEvent O(n²) (`lib/chat/transcript.ts:33-96`); WEBX-9 unknown error codes show server message in main copy; WEBX-10 link-confirm token travels in the login return URL (`channels/link-confirm-view.tsx:43-55`); WEBX-11 Detach presence / Remove binding without confirm (`channels/agent-channels-tab.tsx:166-167`, `:268-270`); WEBX-12 live transcript test gaps; WEBX-13 arbitrary values (`message-group.tsx:118-119`, composer `rounded-[12px]`), hand-rolled alertdialog (`agents/connect-tab.tsx:80-92`); WEBX-14 org-secret list error swallowed in Access tab (`agents/access-tab.tsx:24`)

## WEBX sound: markdown renderer (no raw HTML, http(s) links only, no images), attachments never inlined, icons <img> only, grant editor, token contrast ≥ 4.5:1, plugin approval shows claims in full.
