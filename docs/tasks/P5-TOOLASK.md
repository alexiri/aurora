# P5-TOOLASK: A package can mark a catalogue tool "ask first" by default

## Goal
Owner, 2026-11-19 (D-397). Some tools act on other people: CalDAV's `send_scheduling_message` sends
meeting invitations through the person's server (P5-CALDAV's open item), and outgoing webhooks
(P5-WEBHOOK) post to third parties. Today a package cannot say "ask first" for a tool: it runs without
asking unless a workspace adds it to its ask list (`09`). A package may now declare a per-tool default of
**Ask**; the workspace's and the person's rules still decide, as for any default.

## Change
1. **Manifest (C-1042):** a catalogue tool may carry `approval: ask` (absent = today's behaviour). The
   linter accepts it only on tools of the write group (a read tool cannot default to Ask); additive in
   contract 1 with `min_core`. `crates/aurora-registry/src/manifest/*`, the snapshot (hashes of packages
   without the field unchanged), the linter, fixtures.
2. **Decision (C-1043):** where the gateway evaluates a call's rules, a tool whose package default is
   `ask` is treated as if the effective policy said Ask, **unless** an explicit rule of the workspace or
   organisation allows or blocks that tool (an explicit Allow wins over the package default; an explicit
   Block wins over everything). The approval card names the reason ("this app asks before this action").
   The policy simulator shows it.
3. **Built-in packages:** `caldav` marks `send_scheduling_message` `approval: ask`; check the other
   built-ins for tools that act on other people and propose (don't change without listing them in the
   report). `webhook` (P5-WEBHOOK-1/2) uses it if merged by then.
4. **Dashboard:** the tool's row in the app's access settings shows "Asks first (app default)" and lets
   the workspace override it (Allow) with the existing controls; the Access review shows the default.
5. Docs: `24` (manifest), `09` (CalDAV section: the workaround text is replaced), `06`/`07` (decision
   order), `14`, `API_CONTRACTS` C-1042/C-1043.

## Tests
Registry linter and snapshot; gateway decision ITs (package default Ask → held; workspace Allow → runs;
workspace Block → refused; no default → unchanged); policy simulator; the CalDAV tool held by default in
an IT; web vitest + Playwright mocked for the row and the override.

## Ownership
`crates/aurora-registry/src/manifest/*` (serialize with P5-POLISH-3's registry part and P5-WEBHOOK-1),
the gateway decision, `plugins/builtin/caldav`, the web access settings rows. Size M. D-397, C-1042,
C-1043.

## Completion report
(lane p5-agent, 2026-11-19; gate level: targeted only, CI is the full gate.)

**What changed**
- **Manifest (C-1042):** `ToolItem.approval: Option<ToolDefaultApproval>` (`ask` only, skipped when absent so other packages' hashes hold); lint `tool_approval_read` for a read-group tool; `GET /v1/apps/{app}` `Tool.approval`. `caldav` 1.2.0 marks `send_scheduling_message`; `webhook` marks `send` (not yet on master, so no version bump).
- **Decision (C-1043):** `ToolTags.ask_default`; in `aurora_policy::evaluate`, after the two scopes are combined, `allow` becomes `needs_approval` (`Decision.package_ask`) unless a matched allow rule of either scope **names the tool** in a target's `tools`. Blocks, rate limits, asks, `CONNECT` unchanged.
- **Card / simulator / effective view:** the hold's `rule_name` is "This app asks before this action" (`aurora_policy::eval::PACKAGE_ASK_REASON`, `rule_logical_id` null); `Approval.asked_by` gains `kind: package`; the e-mail drops "Rule:"; simulate adds the trace entry `package_default_ask`; the effective view gives reason layer `package`.
- **Dashboard:** the actions-table row says "Asks first (app default)" and starts as Asks you even under Act; choosing Allowed is the override (a custom grant naming the tool); the card says the app asks.
- Docs: `07 §A package's default of Ask`, `09` (manifest paragraph, CalDAV open item replaced), `14`, `API_CONTRACTS` C-1042/C-1043. The brief's "24 (manifest)" and "06" were not touched: the manifest is documented in `09` and `06` has no decision-order list to amend.

**Decision to confirm (D-397 wording):** "an explicit Allow wins" is implemented as *an allow rule that names the tool*. A whole-connection allow (the **Act** level is mode `full`, one toolless allow rule), a broad `network` allow or the Default Rule's allow do **not** lift the default; otherwise `send_scheduling_message` would send silently at Act and the feature would only matter for rule-less workspaces. Custom grants (Allowed in the table), Always allow and hand rules with a tool list do lift it. **Always allow is not offered on a package-default hold** (no rule to rewrite; the person sets Allowed in the table).

**Other built-ins that act on other people (proposal, nothing changed):** `google-workspace` (Gmail send, Calendar events with attendees), `microsoft-365` (mail send, calendar), `atlassian`/`linear`/`notion`/`gitlab`/`github` (comments and mentions notify people, a merge acts on a repository): candidates for `approval: ask` on mail-send and invitation tools only; not changed.

**Evidence (targeted):** `cargo test -p aurora-policy` (corpus `toolask_01..06`: held with no rule, workspace and organization allow naming the tool lift it, block wins, no default unchanged, a full grant does not lift); `cargo test -p aurora-registry --test it` 162+ passed (linter case, additive-field test, shipped caldav/webhook each ask for exactly one write tool); `cargo test -p aurora-gateway --test it -- ask_tool_hold webhook_send approval_grants` 22 passed (new: held with the app named as reason then approve/deny; explicit allow runs; organization block refuses; no default unchanged); `cargo test -p aurora-api --test it -- simulate_apps caldav_connect approvals:: effective grants_` 85 passed; vitest `accounts`, `approvals`, `approval-card-chat` 43 passed; Playwright `e2e/agent-access.spec.ts` 8 passed (light/dark, 1280/390, axe); clippy `-D warnings` on policy, registry, gateway, api clean; `cargo fmt --check`, `extract-schema --check`, `guards/run-all.sh`, web typecheck clean. Not run: the fail-before check of the new tests (the main gateway case would time out without the evaluator change), a simulate IT for the new trace entry (covered by the shared evaluator and the corpus), full suites.
