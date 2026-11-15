# P5-UX-7: A simple layer over Credentials and Policy (owner walkthrough, 2026-11-15)

## Goal

The walkthrough's workspace owner met operator vocabulary on two pages:

- **Credentials** has six tabs: Secrets, LLM keys, Proposals, Apps, Connections and OAuth clients.
- **Policy** opens on "No default (defer)", "Uncredentialed requests", "Browser tunnels (open lane)",
  "Speaker" and "Lane". Its empty state contradicts its own default.

This task adds a plain layer on top of each page and keeps every existing control one click away. It
makes no API change and no policy-semantics change.

## Linked requirements

- D-327; contracts **C-925, C-926**.
- Normative: `14 §First run and connecting` §6, `14` workspace Credentials (~1370–1420) and Policy
  sections, `07 §Evaluation` and `§Authoring` (semantics the summary must state truthfully), `08 §5`
  (secret recipes), `14 §Visual design`.

## Context

- **Credentials.** `web/src/components/credentials/credentials-view.tsx` (tabs ~44–50; Proposals and
  Apps only at workspace scope), `generic-form.tsx` (~122–171), `proposals.tsx`, `connections-tab.tsx`,
  `apps-tab.tsx`. URL tabs `?tab=secrets|llm|proposals|apps|connections|oauth`, with Secrets the fallback.
- **Policy.** `web/src/components/policy/policy-view.tsx` (default rule ~312–315, uncredentialed
  ~334–343, tunnels ~348, Apply/Discard ~427–433, empty state ~240), `simulate-panel.tsx` (~109–164).
- **What actually happens by default** (`07 §Evaluation`, `08 §11.6`): in a fresh workspace with posture
  `open` and no rules, an uncredentialed request is **allowed**, and a credentialed request with no
  matching rule is allowed. The default rule applies only where a credential is injected. The summary
  must say exactly this and nothing looser.

## Scope

1. **Credentials › Overview (C-925).** A new first and default tab `overview`.
   - One `ItemList` over the workspace's connections, generic secrets and LLM keys. Each row: icon/name,
     kind chip (App, Service, LLM key), agent avatars with access (from `grant_count` and the grants
     queries; "No agent", with the P5-UX-4 badge), access (Read only / Asks before changes / Full / —)
     and a row menu (Give access, Rename, Replace value or Reconnect, Delete).
   - **Add** (a `Menu`):
     - **An app**: the P5-UX-3 journey with the agent step;
     - **A service**: a short form of Name, Website (host) and Token, using the Bearer recipe. Its
       **Advanced** holds the path pattern and the other `08 §5` recipes, reusing `generic-form.tsx`
       pieces;
     - **An LLM key**: the existing LLM form.
   - The Proposals tab shows only when proposals wait, with a count `Badge`, plus a banner on Overview.
   - OAuth clients leaves the tab bar: a foot link "App sign-in setup (OAuth clients)" for `can_own`.
     `?tab=oauth` still opens it.
   - The other tabs are unchanged.
2. **Policy summary and Common changes (C-926).**
   - `web/src/lib/policy/summary.ts`: a pure function from (effective org posture, workspace defaults,
     rules, tunnels setting) to an ordered list of sentences. Each sentence's condition is in a table
     the unit tests walk, and every sentence must be true under `07 §Evaluation`. Have the reviewer check
     the table against `07`, not against this brief.
   - Render it at the top of the workspace Policy page.
   - **Common changes:** buttons that open the existing rule editor prefilled as a draft, then the
     existing **Apply changes**:
     - ask before `POST|PUT|PATCH|DELETE` to a host;
     - block a host;
     - only allow listed websites (uncredentialed `block`).
3. **Wording (C-926).**
   - "No default (defer)" → **Let the organization decide**
   - "Uncredentialed requests" → **Requests without a credential**
   - "Browser tunnels (open lane)" → **Browser connections (not inspected)**
   - Simulate → **Test a request**, with Speaker and Lane under **More**
   - The empty-rules sentence becomes "Until you add one, requests follow the defaults below."

   Update the help texts next to them so they stay accurate. Keep the org Policy page's labels identical
   to the workspace ones.
4. **Tests.**
   - Vitest: the summary table (each posture/default/rule combination, including the fresh-workspace
     case → "Agents may reach any website. Nothing asks you first."); the Overview list grouping and the
     menus; the service form payload (Bearer by default, Advanced recipes unchanged); the Proposals tab
     visibility.
   - Playwright: add a service from Overview in the one-agent workspace; it appears with the agent's
     avatar (after P5-UX-4's preselection). A Common change creates a draft and applies it.
   - Axe and narrow-route suites, both themes.

## Constraints / file ownership

- Owns: `credentials-view.tsx` (tabs), the new `credentials-overview.tsx`, `generic-form.tsx`
  (Advanced split), `policy-view.tsx`, `simulate-panel.tsx`, `lib/policy/summary.ts`.
- Reads P5-UX-3's journey and P5-UX-4's badge and preselection. Run after both.
- No API or policy-semantics change. If a needed count is missing, stop and report; do not add a route.

## Acceptance criteria

1. A workspace owner can see on one screen which credentials exist and which agents can use them, and can
   add an app, a service or an LLM key from it.
2. The Policy page's first lines describe the workspace's real behaviour, and the summary tests pass
   against the `07` cases.
3. No label in the walkthrough's list ("No default (defer)", "Uncredentialed requests", "open lane",
   "Simulate") remains on the workspace or org Policy page.
4. Every previous tab and control is still reachable (deep links included); existing tests pass or are
   updated only for label text.
5. Web gates; axe and narrow-route suites, both themes.

## Dependencies

P5-UX-3 and P5-UX-4 merged.

## Suggested agent/model and reason

Sonnet (`nextjs-developer`). The summary's truthfulness against `07` is the risky part and needs
judgement; the rest is UI composition.
