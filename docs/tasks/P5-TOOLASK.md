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
