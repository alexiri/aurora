# P5-POLISH-10: Administration: setup checklist, apps from Aurora, unavailable features, settings pages, endpoint warnings (owner, 2026-11-19)

## Goal

Make Administration a place an instance admin can finish setup in and then trust. UX review 4 found:

- **Set up Aurora:**
  - step 2 embeds the whole placement form (about 1,200 px);
  - adding the AI key flips the page to the health view with no message, so "Try it" is never seen;
  - "Optional" says READY next to two "Check this" lines;
  - bare numbers sit under check lines;
  - three amber treatments on one step;
  - twelve apps wait for approval with no way through but one at a time.
- **Apps:**
  - the list shows package ids ("aws-keys", "google-workspace") and versions, with amber "PENDING
    APPROVAL" caps on every row;
  - the subtitle says "Signed packages" while every row says "unsigned";
  - the review page keeps Approve disabled until all five collapsed sections are opened, and shows raw
    codes ("(first_install)", "{instance}");
  - clicking a package name in the list did not navigate in one test.
- **Unavailable features:**
  - Computers and Directory sync show a notice that the feature can't be turned on, then the whole
    disabled form (P5-POLISH-2 builds the `Unavailable` card and replaces those two);
  - Chat platforms says "fake is added, but waiting for the connector host. The operator must start
    it. You are an instance administrator.".
- **Sign-in:**
  - it says "No provider yet. Add one so people can sign in." while people sign in through the
    Helm-defined development provider;
  - break-glass CLI text is the first thing on the page.
- **People › Users:** the admin's own row offers "Remove admin" and "Turn off".
- **AI models:** endpoints carry chip soup (LITELLM, amber MANAGED BY HELM, CLAUDE CODE, DEFAULT) and
  amber "Set the roles." notices on Helm-managed endpoints that can't be changed here. The role warnings
  belong here, once per endpoint (D-385), not on every agent.

## Linked requirements

- **Design reference (D-380).** Proposals **E4**, **E5**, **E6** and **E7** (Administration parts) of
  “Aurora UX fixes”, <https://claude.ai/artifact/WXbcpcrkbCLuxMSurpkuQW> (`#e4` … `#e7`).
  - Local copy `/home/alex/orca/workspaces/aurora/design-ux-fixes/`: `after/e4.png`, `after/e5.png`,
    `after/e6.png`, `after/e7.png`, the `before/` crops (`e4`, `e4b`, `e5`, `e5b`, `e6`, `e7`), and
    `after/src/<id>.html`.
  - The owner's comments, applied in the artifact:
    - E5 and E4: apps shipped with Aurora arrive approved, so setup has **five** steps (no approval
      step), and E5 lists "From Aurora · 12 apps, approved with this release";
    - E7: the setting "Agents Aurora runs for people" was "absolutely tortured language". It is now
      **Run agents on Aurora's computers**: "On: Aurora gives each agent its own computer to work on.
      Off: people can only connect agents that run on their own machines.".
  - The review: <https://claude.ai/artifact/33oHaXQpCU3oSVRVmgtY5Z> (screens 010–052).
  - Yellow dashed labels are commentary.
- **Decisions:** **D-391** (checklist), **D-381** (builtin approval; server in P5-POLISH-3), **D-392**
  (settings rows, Unavailable), **D-384** (lock pill), **D-385** (endpoint warnings).
- **Contracts:** **C-1032** (`origin`, `summary`, `reaches` on the plugins list), **C-1034**
  (`GET /v1/admin/llm-endpoint-warnings`).

## Wave and order

- After **P5-POLISH-2** (pieces) and **P5-POLISH-3** (data), in lane C: P5-POLISH-8 →
  **P5-POLISH-10**.
- Independent of P5-POLISH-4, -5, -6, -7 and -9.

## Context

All paths are under `web/src/`.

- **Administration topics:** `components/admin/admin-topics.tsx` (Sign-in :140–163, All settings :216).
- **Overview / setup:** `components/admin/setup-view.tsx`:
  - `SetupView` :477;
  - `setupComplete` :354 decides when the health view shows (around :486);
  - `HealthView` :380;
  - `StepCard` :113, steps 1–6 at :526–546, title :500;
  - step 2 embeds `<PlacementView embedded>` (`components/admin/placement-view.tsx`).
  - The API is `GET /v1/admin/setup` (`05`: steps with `ok | warning | todo` and reason codes, C-922,
    C-924).
- **Apps:**
  - list `components/admin/plugins-view.tsx:132` (`statusTone` :45, the table :179);
  - review `components/admin/plugin-detail-view.tsx` (`seen`/`allSeen` gate :111, :126; Approve
    disabled :359).
- **Settings:**
  - `components/admin/settings/*` (`operational-settings-views.tsx`: Computers :390, `terminalNotice`
    :349, `OperationalStack` :203; `sign-in-settings-view.tsx`: "No provider yet" :139, Helm chip :80,
    `section("scim")`; `llm-controls.tsx` :244);
  - `components/ui/setting-field.tsx` (rebuilt by P5-POLISH-2).
- **Chat platforms:** `components/connectors/connectors-admin-view.tsx:23`.
- **Users:** the admin People › Users view (find it from `admin-topics.tsx`).
- **AI models:** endpoints in `components/governance/models-tab.tsx` and `llm-controls.tsx`. The
  effective settings `eff.llm.endpoints` come from `lib/api/governance.ts:210`.

## Scope

1. **Setup checklist (E4, D-391).** As in `after/e4.png`:
   - **Title** "Set up Aurora" and a sentence ("Five steps from a fresh install to your first
     answer."); "<n> of 5" and a progress meter.
   - **Five rows**, each a `LinkCard`-style row with a status (a check in an accent circle when done,
     the step number otherwise), a title, a one-line summary of the current state, and **one** button:

     | Row | Summary | Button |
     |---|---|---|
     | **Sign-in** | "Development sign-in", or the provider names | **Review** → People › Sign-in |
     | **Where new people land** | "The Users organization, each in their own workspace" (from the placement) | **Change** → People › New users. No embedded form |
     | **AI key** | "<key name> · every new agent gets it", or "Add an organization AI key" | the AI-key dialog (P5-POLISH-7's dialog, organization scope) |
     | **Try it** | "Say hello to your own agent" | **Open chat** → the admin's own agent's Chat |
     | **E-mail · optional** | "For invitations and approval notices" | **Set up** |

     - **Try it** is done when the admin's agent has answered once (the existing onboarding state).
     - **E-mail** is done when SMTP works. An optional step shows its own state; it never says Ready
       while it has an open item.
   - Remove the approval step: builtin apps are active (D-381). If uploaded apps wait, a line under
     the checklist says "1 uploaded app is waiting for review" and links to Apps.
   - **Progress:** when a step completes (for example the AI key is saved), show a toast ("AI key added.
     3 of 5 done.") and stay on the checklist.
   - **The health view** (Overview) replaces the checklist only when every required step is done, or
     from a **Health** tab that is always there. **Run setup again** on the health view returns to the
     checklist.
   - No bare numbers under check lines. Each line is a sentence.
   - The "Web Push · Off" strip goes into the E-mail row's summary or the health view, not between the
     title and step 1.
2. **Health view.**
   - Use `StatusLine` rows.
   - One sentence per problem. "E-mail isn't set up. Aurora can't send approval or invitation e-mails."
     appears once, with **Set up e-mail**.
   - "1 runner online.", not "runner(s)".
   - If the AI-models health line reports an endpoint "did not answer" right after a key on it was
     accepted, find out why: the probe may use a different key, or the dev mock may not answer the
     probe. Fix it, or explain it in the completion report.
3. **Apps (E5).** As in `after/e5.png`:
   - **The list:** a sentence ("1 app is waiting for your approval." or "No app is waiting."), then
     **Waiting** (uploaded, directory and addon packages pending review) as rows: icon, name, "<origin>
     · <signed|unsigned> · reaches <host>", **Review**.
   - Then **From Aurora**: "<n> apps, approved with this release", with the first three rows (icon,
     name, "Reaches <host> · needs <a token | Google sign-in | …>", **Details**) and "<n> more"
     expanding the rest.
   - Use `origin`, `summary` and `reaches` (C-1032). Package ids, versions, sources and signers move to
     the review page. Dates use `formatWhen`.
   - **Upload app** replaces "Upload plugin". The subtitle drops "Signed packages".
   - **The review page:**
     - all sections are open by default;
     - **Approve** is enabled from the start (remove the `seen`/`allSeen` gate);
     - the title is the app name with the version muted;
     - raw codes render as sentences: "(first_install)" → "First install"; `{instance}` → "your
       server's address"; capability ids such as `token.mint` get their human labels where the review
       already has them;
     - after **Approve**, show a toast and return to the list.
   - Make sure the package name link navigates (the review found it didn't in one test). Use a real
     `<Link>`, and add a Playwright check.
4. **Unavailable features (E6).** Beyond P5-POLISH-2's Computers and Directory sync:
   - **Chat platforms:** a registered connector whose host isn't running shows one `Unavailable`-style
     card: "<Platform display name> is added, but its connector isn't running", with one sentence for
     the operator and a guide link. Never the connector's raw id ("fake") as a name, and no "You are
     an instance administrator." sentences.
   - "Add Mattermost" stays where an admin can act.
5. **Settings pages (E7)** on top of P5-POLISH-2's rows:
   - The runners table spans the card with all columns visible at 1280 px.
   - Rename the one setting the owner asked for: **Run agents on Aurora's computers**, with the help
     text above (keep the stored key).
   - No raw enum values: use the same value-to-words approach as P5-POLISH-9 for the admin BYO
     settings.
   - **Sign-in:**
     - list the providers from Helm as read-only rows with `LockPill` "Set in Helm" (the development
       sign-in shows as "Development sign-in · Set in Helm"), so "No provider yet" appears only when
       there really is none;
     - move the break-glass text into a collapsed "If nobody can sign in" disclosure at the end of the
       page;
     - one Save per form section, at that section's end, only when it has changes.
6. **AI models (C-1034, D-385).**
   - Each endpoint row: the name, its kind as plain text ("LiteLLM gateway"), the models line, and at
     most one chip. A Helm-managed endpoint shows `LockPill` "Set in Helm".
   - Endpoints with role warnings (C-1034) show one line with an action: "Agents may ask for a model
     this endpoint doesn't serve: set the Large, Medium and Small roles." with **Set roles**, which
     opens the roles fields. For Helm-managed endpoints, the line says where to set them ("Set in the
     Helm values under `llm.endpoints.<id>.roles`") and is neutral, not amber.
   - Remove the "Set the roles." notices that repeat per agent.
7. **People › Users.**
   - Your own row offers no "Remove admin" or "Turn off". The last instance admin can't be removed
     (hide it; the server refuses already).
   - Role values move into a per-row … menu.
8. **Tests:**
   - **Vitest:**
     - the checklist states (each step's done and not done, the optional step never Ready with open
       items);
     - the toast and staying on the checklist after the AI key;
     - the health view only when complete, or from the tab;
     - the apps list groups;
     - Approve enabled without expanding;
     - Chat platforms' card;
     - Sign-in shows Helm providers;
     - endpoint warnings;
     - own-row rules in Users.
   - **Playwright** (mocked), 1280 and 390 px, both themes, axe, no console errors:
     - a fresh install walks the five steps;
     - adding the AI key stays on the checklist with the toast;
     - the apps list navigates to a review;
     - Approve works without expanding sections.

## Out of scope

- Server changes (P5-POLISH-3 provides `origin`, `summary`, `reaches` and the endpoint warnings).
- The organization pages (P5-POLISH-9).
- The general wording pass. Only the setting the owner named is renamed here.

## Constraints / file ownership

- **Owns:**
  - `components/admin/**` except the shared settings field and section form, which P5-POLISH-2 owns
    (page composition is yours);
  - `components/connectors/connectors-admin-view.tsx`;
  - the endpoint rows in `components/governance/{models-tab,llm-controls}.tsx` (admin view only; the
    member view is P5-POLISH-9's).
- Uses `LinkCard`, `StatusLine`, `LockPill`, `Unavailable`, `Switch` and the AI-key dialog. Doesn't
  change them.

## Acceptance criteria

1. A fresh dev install shows the five-step checklist. Adding the AI key stays on it with a toast. The
   health view appears only after the required steps (`after/e4.png`).
2. Apps lists "From Aurora · 12 apps, approved with this release" and only uploaded packages under
   Waiting. A review's Approve is enabled at once (`after/e5.png`).
3. Chat platforms, Computers and Directory sync show cards, not disabled forms (`after/e6.png`).
4. Settings pages use the full width. Sign-in shows the Helm provider. "Run agents on Aurora's
   computers" reads as written (`after/e7.png`).
5. AI models shows each endpoint's role warning once, with its fix. No agent page shows role warnings.
6. Web gates: typecheck, lint, vitest, build, Playwright mocked (axe, narrow-route, both themes).
   Report the counts.

## Suggested agent/model and reason

Sonnet (`nextjs-developer`). Several Administration pages with state-dependent rendering.
