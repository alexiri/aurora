# P6-SKLIB-HIDE: an organization hides a shared (instance) skill library — **proposal, phase 6**

> **Status: proposal (2026-11-21).** Owner, 2026-11-21, answering Q-SKLIB-1 ("may an organization hide
> an instance library from its people?"): "not yet, write brief to improve this later". P5-SKLIB-2 ships
> with every organization seeing every active instance library (D-449(b)). Nothing here is decided. No
> D-, C-, SEC- or migration numbers are taken yet; the spec batch takes them from PROGRESS's next-free
> note. Open questions are `Q-P6-HIDE-n` at the end; the recommendations are the coordinator's. Depends on
> P5-SKLIB-2 and -3 being merged. Don't start until the owner says so.

## Goal

An instance admin curates libraries for every organization (D-449). An organization may not want some of
them in front of its people: a library for another department, one its policy forbids, or simply noise in
Add from library. Its owners and admins should be able to hide an instance library for their organization
only.

## Today (after P5-SKLIB-2/-3)

- `skill_libraries.org_id` null = instance library; managed at `/v1/admin/skill-libraries`.
- `skill_library_org_state (library_id, org_id, …)` (migration `20261201000271`) holds per-organization
  state of an instance library (today `over_budget`).
- `GET /v1/orgs/{org}/skill-libraries` lists the org's libraries plus every active instance library with
  `scope: "instance"`; browse and content take `?org=`; Add from library lists both groups.
- Dashboard: Agents › Skills › Libraries, section "Shared with every organization" (read-only, Browse).

## Proposal

1. **State.** A `hidden_at` / `hidden_by` pair on `skill_library_org_state` (a row is created when an
   organization hides a library and keeps its `over_budget` meaning).
2. **Who.** The organization's owners and admins (the org tier's write authority, as for its own
   libraries); session or an org API key with the same scope as library management. Never members.
3. **Routes.** `PUT` / `DELETE /v1/orgs/{org}/skill-libraries/{id}/hidden` on an instance library (404 on
   an organization library or one the caller cannot read, as the P5-SKLIB-2 helper answers).
4. **Effect for the organization.**
   - Owners and admins still see it in the libraries list, marked Hidden, with Show again.
   - Members don't see it in the list, in Add from library or in browse (browse with `?org=` answers 404).
   - Skills already added from it stay as they are (see Q-P6-HIDE-1) and keep their badge.
   - Refresh writes no new update proposals for this organization while hidden (see Q-P6-HIDE-2).
5. **Instance admin view.** "Organizations using it" also shows how many organizations hid it, never
   which skills they have (D-449(c)).
6. **Audit.** `skill_library.hide` / `skill_library.unhide` in the organization's audit log with
   `{ library_label, repo }`.
7. **Dashboard.** On the "Shared with every organization" rows: Hide / Show again for owners and admins,
   a Hidden status in words, and the Add from library and browse lists filtered for everyone else.
   Tokens and shared primitives only, both themes, 390 px.

## Security notes (for the spec pass)

- Hiding only narrows what an organization's people see; it never grants anything. The P5-SKLIB-2
  authority helper stays the single gate, with "hidden for this org" added as a reason to answer 404 to
  non-admins.
- An organization's choice is invisible to other organizations; the instance admin sees only a count.

## Open questions

- **Q-P6-HIDE-1**: skills already added from a library the organization then hides: keep them
  (recommended: hiding is about what's offered, not about breaking agents), detach them (they keep their
  content, like a delete), or ask in the Hide dialog?
- **Q-P6-HIDE-2**: while hidden, do already-added skills still get update proposals? Recommended: no (the
  organization said it doesn't want the library); they resume on Show again.
- **Q-P6-HIDE-3**: should an instance admin be able to mark a library **required** (it can't be hidden),
  for example a security guidance library? Recommended: not in this brief.
- **Q-P6-HIDE-4**: hiding per workspace as well? Recommended: no; workspaces already choose skill by skill.
