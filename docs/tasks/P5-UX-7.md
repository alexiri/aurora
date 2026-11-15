# P5-UX-7: A simple layer over Apps & services and Internet access (owner walkthrough, 2026-11-15)

## Goal

The walkthrough's workspace owner met operator vocabulary on two pages:
- **Credentials** had six tabs: Secrets, LLM keys, Proposals, Apps, Connections and OAuth clients.
- **Policy** opened on "No default (defer)", "Uncredentialed requests", "Browser tunnels (open lane)",
  "Speaker" and "Lane", and its empty state contradicted its own default.

In the new structure (D-338) those pages become **Apps & services** (with LLM keys moving to **AI
models**) and **Internet access**. This task gives each a plain first layer: an **Overview** of what agents
can use, and a **summary with Common changes** of what agents may reach. Every existing control stays one
click away. There is no API change and no policy-semantics change.

## Linked requirements

- D-327, D-342; contracts **C-925, C-926**.
- Normative:
  - `14 §First run and connecting` §6;
  - `14 §Information architecture` (Apps & services and Internet access rows, the vocabulary, the scope
    ladder);
  - `07 §Evaluation` and `§Authoring` (the semantics the summary must state truthfully);
  - `08 §5` (service recipes);
  - `14 §Visual design`.

## Placement in the new structure (D-342)

- **Overview** is the default tab of the workspace's **Apps & services** page (`…/apps?tab=overview`). The
  page and its tabs come from P5-IA-1: Overview · Apps · Connections · Services · OAuth clients
  (`can_own`) · Proposals (when waiting).
- **The summary and Common changes** are the top sections of the workspace's and the org's **Internet
  access** page, which P5-IA-2 composes. This task supplies `summary.ts`, the Common changes and the label
  renames; P5-IA-2 places them in its order. Whichever of the two merges second wires them together.

## Context

- **Apps & services (today's Credentials).** `web/src/components/credentials/credentials-view.tsx`,
  `generic-form.tsx` (~122–171), `proposals.tsx`, `connections-tab.tsx`, `apps-tab.tsx`.
- **Internet access (today's Policy).** `web/src/components/policy/policy-view.tsx` (default rule
  ~312–315, uncredentialed ~334–343, tunnels ~348, Apply/Discard ~427–433, empty state ~240) and
  `simulate-panel.tsx` (~109–164).
- **What actually happens by default** (`07 §Evaluation`, `08 §11.6`). In a fresh workspace with posture
  `open` and no rules:
  - an uncredentialed request is **allowed**;
  - a credentialed request with no matching rule is allowed;
  - the default rule applies only where a credential is injected.

  The summary must say exactly this and nothing looser.

## Scope

1. **Apps & services › Overview (C-925).**
   - **The list.** One `ItemList` over the workspace's connections and services (generic secrets). Each row
     has:
     - icon and name;
     - a kind chip (App, Service);
     - agent avatars with access (from `grant_count` and the grants queries; "No agent", with P5-UX-4's
       badge);
     - access (Read only / Asks before changes / Full / —);
     - a row menu: Give access, Rename, Replace value or Reconnect, Delete.
   - **AI models line.** One line under the list links to **AI models**: "<n> LLM keys, used by <m>
     agents".
   - **Add** (a `Menu`):
     - **An app**: the P5-UX-3 journey with the agent step;
     - **A service**: a short form of Name, Website (host) and Token, using the Bearer recipe. Its
       **Advanced** holds the path pattern and the other `08 §5` recipes, reusing `generic-form.tsx`
       pieces.
   - **Proposals** shows as a tab only when proposals wait, with a count `Badge`, plus a banner on
     Overview.
   - The Services tab is the generic-secret list renamed. The other tabs are unchanged in content.
2. **Internet access summary and Common changes (C-926).**
   - `web/src/lib/policy/summary.ts` is a pure function from (the scope ladder's above values, this level's
     defaults, the rules, the browser-connections setting) to an ordered list of sentences.
   - Each sentence's condition is in a table the unit tests walk, and every sentence must be true under
     `07 §Evaluation`. Have the reviewer check the table against `07`, not against this brief.
   - **Common changes** are buttons that open the existing rule editor prefilled as a draft, then the
     existing **Apply changes**:
     - ask before `POST|PUT|PATCH|DELETE` to a host;
     - block a host;
     - only allow listed websites (requests without a credential: `block`).
3. **Wording (C-926)**, on the workspace and org Internet access pages alike:
   - "No default (defer)" → workspace: **Let the organization decide**; org: **None: only rules decide**
     (there is no instance rule scope, so nothing above the org decides; a request no rule matches is
     allowed, `07 §Evaluation`)
   - "Uncredentialed requests" → **Requests without a credential**
   - "Browser tunnels (open lane)" → **Browser connections (not inspected)**
   - Simulate → **Test a request**, with Speaker and Lane under **More**
   - The empty-rules sentence becomes "Until you add one, requests follow the defaults below."

   Update the help texts next to them so they stay accurate.
4. **Tests.**
   - Vitest:
     - the summary table (each posture, default and rule combination, including the fresh-workspace case
       → "Agents may reach any website. Nothing asks you first.");
     - the Overview list grouping and the menus;
     - the service form payload (Bearer by default, Advanced recipes unchanged);
     - Proposals tab visibility.
   - Playwright:
     - add a service from Overview in the one-agent workspace; it appears with the agent's avatar (after
       P5-UX-4's preselection);
     - a Common change creates a draft and applies it.
   - Axe and narrow-route suites, both themes.

## Constraints / file ownership

- **Owns:**
  - the new `apps-overview.tsx`;
  - `generic-form.tsx` (Advanced split and "service" wording);
  - `policy-view.tsx` (labels and the empty sentence);
  - `simulate-panel.tsx` (labels);
  - `lib/policy/summary.ts`;
  - the Common changes component.
- **Does not own** the topic pages or their tab bars (P5-IA-1) or the Internet access composition
  (P5-IA-2).
- **Reads** P5-UX-3's journey and P5-UX-4's badge and preselection. Run after both, and after P5-IA-1.
- **No API or policy-semantics change.** If a needed count is missing, stop and report; do not add a route.

## Acceptance criteria

1. A workspace owner can see on one screen which apps and services exist and which agents can use them,
   and can add an app or a service from it. LLM keys are one link away.
2. The Internet access pages' first lines describe the real behaviour at that level, and the summary tests
   pass against the `07` cases.
3. No label in the walkthrough's list ("No default (defer)", "Uncredentialed requests", "open lane",
   "Simulate") remains on any Internet access page.
4. Every previous tab and control is still reachable (deep links included). Existing tests pass, or are
   updated only for label text and routes.
5. Web gates; axe and narrow-route suites, both themes.

## Dependencies

P5-IA-1 (topic pages and routes), P5-UX-3, P5-UX-4. P5-IA-2 is soft (see Placement).

## Suggested agent/model and reason

Sonnet (`nextjs-developer`). The summary's truthfulness against `07` is the risky part and needs judgement;
the rest is UI composition.
