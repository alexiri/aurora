# P5-AGENT-2: agent page in five sections, with a collapsible status rail

## Goal
Rebuild the agent page as specified in `14 §Screens` **Agent page (phase 5)**: a one-line header, the thirteen tabs grouped into five sections (D-200), and in the Chat section a status rail that collapses to an icon strip (D-201). Page contents behind each tab are unchanged.

## Linked requirements
- Master spec IDs/sections: D-200, D-201, C-700; `14 §Screens` "Agent page (phase 5)" (normative, read it whole) and "Agent chat"; `14 §Visual design` (tokens, primitives, both themes, 390 px); `14 §Information architecture` agent page tabs.
- Design reference: the "Agent page redesign" canvas, boards **Chosen · Rail expanded**, **Chosen · Rail collapsed** (press Play: collapse/expand and the approvals pop-over work) and **A · Phone**; the Aurora design system artifact for primitives. Where a board and `14` disagree, `14` wins. The boards' "+ New conversation" button is **not** built (a person cannot create conversations; direct chats are get-or-create).
- Requirement summary: chat-first agent page; configuration in four named groups; live state (approvals waiting, work in progress, recent tool calls, sandbox) visible beside the chat, collapsible, remembered per browser.

## Context
- Page: `web/src/components/agents/agent-page.tsx` (tabs list, `useTabParam`, `SandboxPill`, `AgentFreeze`, `DeleteZone`).
- Chat: `components/agents/chat-tab.tsx`, `components/chat/{chat-panel,conversation-list,chat-thread,approval-card,thread-header}.tsx`.
- Data hooks: `lib/api/conversations.ts` (`useConversations` — rows carry `active_turn`; `useSandbox`), `lib/api/approvals.ts` (`usePendingApprovals`, `useCanApprove`, `useActivity`, decide mutation, approvals stream), action approvals hooks.
- Activity page: `components/approvals/activity-view.tsx` (agent filter is component state today).
- Primitives: `components/ui/*` (`Tabs` incl. FIX-129's overflow, `Menu`, `Dialog`, `StatusDot`, `Badge`, `Button`, icons). Theme storage pattern: `components/theme/theme.ts`.
- API: C-700 (`agent_id` on `GET /v1/workspaces/{ws}/approvals`, from P5-AGENT-1); existing `GET …/action-approvals?status=pending&agent_id=`, `GET …/activity?agent_id=&limit=`, `GET /v1/approvals/stream`, the sandbox endpoint.

## Scope
### In scope
1. **Sections** (D-200): a single mapping module (e.g. `components/agents/sections.ts`) page id → section; header switcher (segmented `Tabs`, "Agent sections"); per-section segmented row of its pages; `?tab=<page>` kept with today's ids, `?c=` still opens Chat; default page Chat (hosted) / Instructions (BYO); hidden sections when no page is available (BYO: Behaviour = Instructions, Computer = Computer: Connect, Access).
2. **Header**: one line that wraps, as `14` says; Agent actions menu with Freeze/Unfreeze (reuse `AgentFreeze` `inMenu`) and "Delete agent…" (reuse the `DeleteZone` flow in a dialog: same typed confirmation, same permission, same redirect); remove the Delete zone under Instructions; frozen banner unchanged.
3. **Chat panel**: list (232 px) · thread · rail; height keeps "no page scroll at 1280×800". List rows show "Waiting on you" (`pending` dot) for approvers when the conversation has a pending item. The inline approval card gets an `id` the rail links to.
4. **Status rail** (D-201): the five parts, their data, refresh rules, expanded/collapsed states, indicators with accessible names, the Needs-you pop-over, persistence per browser (`localStorage`, try/catch, defaults by viewport: ≥ 1440 px expanded), overlay mode when the panel is < 1024 px wide, approvers-only Needs you.
5. **Below 768 px**: no rail; "N approvals waiting" banner + header "Status" button opening the parts in a `Dialog` sheet; list/thread two-view behaviour unchanged.
6. **Primitives**: add `ChevronLeftIcon` to `ui/icons.tsx`; add `ui/popover.tsx` (Radix Popover, exact version pinned like the other Radix packages, `package.json` + `pnpm-lock.yaml`); document both in `14 §Visual design` component list if the list names primitives.
7. **Activity link**: `activity-view.tsx` initialises its agent filter from `?agent=<id>` (and writes it back), so "All activity" lands filtered.
8. Tests (below).

### Out of scope
- Redesigning the contents of Behaviour, Triggers, Computer or Access pages (later briefs).
- Any API change (P5-AGENT-1 owns C-700). If a needed field is missing, stop and report; do not invent an endpoint.
- A "new conversation" action; server-side storage of the rail preference.

## Constraints / file ownership
- Owns: `web/src/components/agents/agent-page.tsx`, new `web/src/components/agents/{sections.ts,status-rail/**}`, `components/agents/chat-tab.tsx`, `components/agents/delete-zone.tsx` (only to expose the flow as a dialog), `components/chat/{chat-panel,conversation-list,approval-card}.tsx`, `components/approvals/activity-view.tsx`, `components/ui/{icons.tsx,popover.tsx}`, new hooks in `lib/api/approvals.ts`, `web/package.json`, `web/pnpm-lock.yaml`, tests under `web/tests/` and `web/e2e/` named below.
- Not in parallel with another lane that edits `package.json`/lockfile (FIX-137) or `agent-page.tsx`/`terminal-tab.tsx` (FIX-138).
- Tokens and primitives only (`14 §Visual design`); no inline `style` (production CSP); both themes; no horizontal page scroll at 390 px.
- Backend stays authoritative: hiding Needs you from non-approvers is UX; decisions still go through `POST /v1/approvals/{id}/decision` / action-approval decisions with their 403/409 handling.
- Keep every existing `?tab=` value working; tests that assert today's tab ids are updated only where the visible label moved, never to drop a deep link.

## Acceptance criteria
- [ ] Hosted agent: header one line at 1280 px; switcher shows Chat · Behaviour · Triggers · Computer · Access; default is Chat; `?tab=memory` opens Behaviour › Memory; `?tab=access`, `?tab=instructions`, `?c=<id>` open what they open today; BYO shows Behaviour (Instructions) · Computer (Computer: Connect) · Access and opens on Instructions.
- [ ] Terminal/Previews pages appear only where enabled; a section with no available page is not rendered.
- [ ] Freeze and Delete are in Agent actions with today's confirmations and permissions; no Delete zone under Instructions.
- [ ] Chat at 1280×800: no page scroll; list, transcript and rail scroll inside; rail expanded by default at 1440 px wide, collapsed at 1280 px when nothing is stored; the choice persists across reloads (and is not sent to the API).
- [ ] Collapsed strip: five icons in order; count on Needs you; dots for working / denied-or-failed / sandbox state; each icon's accessible name states the state in words; the Needs you icon opens and closes the pop-over (Esc closes, focus returns).
- [ ] Needs you lists this agent's pending gateway (C-700) and action approvals, soonest first, live on stream events; the open conversation's item says "Show in conversation" and focuses the inline card; Approve/Deny elsewhere work; 409 shows "Already decided."; non-approvers see no Needs you part, icon, banner or "Waiting on you" line.
- [ ] Doing now, Recent tool calls (5 rows, decision badges, "All activity" → Activity filtered by `?agent=`), Computer and About match `14`.
- [ ] Panel < 1024 px: the expanded rail overlays the thread (focus-trapped, Esc closes).
- [ ] < 768 px: no rail; banner and Status button open the sheet; no horizontal scroll at 390 px.
- [ ] axe clean on the agent page in both themes (Chat with rail expanded and collapsed, one Behaviour page).

## Verification
- Commands (from `web/`): `pnpm typecheck`; `pnpm lint`; `pnpm test` (incl. new `tests/agent-sections.test.ts` — mapping and legacy `?tab=` ids, `tests/status-rail.test.tsx` — parts, indicators, accessible names, persistence/defaults with storage throwing, approver gating, pop-over); update `tests/agent-page-header.test.tsx`, `tests/chat-v2.test.tsx`; `pnpm build`; Playwright `e2e/chat-v2.spec.ts`, new `e2e/agent-page-p5.spec.ts` (1280×800 no page scroll, rail toggle persists, 1440 default, 390 px no horizontal scroll, approve from the rail), `e2e/stage4-routes.spec.ts`, `e2e/stage4-axe.spec.ts` (both themes).
- Expected results: all green; screenshots of the four states (expanded, collapsed, pop-over open, phone sheet) attached to the completion report.

## Dependencies
- Blocked by: P5-AGENT-1 (C-700) for Needs you; FIX-129 and FIX-099/FIX-117 merged (same files); not concurrent with FIX-137 (lockfile) or FIX-138 (agent page/terminal).
- Unblocks: the later per-section redesign briefs (Behaviour, Triggers, Computer, Access) and the agent-page phone views.

## Suggested agent/model and reason
`nextjs-developer` (Sonnet): multi-file UI with state, persistence, accessibility and live data; Opus reviews the result against `14` and the boards.

## Completion report
- Files changed:
- Tests/checks:
- Remaining risks:
