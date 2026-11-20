# P5-REFINE-17: Organization: a Manage area, an overview, agents, invitations, models and rules (owner, 2026-11-20)

## Goal

UX review 5 walked Acme as its owner. What is wrong now:

- **Broken.** Chat platforms says "Mattermost is added, but its connector isn't running" while Administration shows its host
  running. Apps › Set up on Google Drive changes the URL but keeps showing Apps. `/o/new` renders a fake organization named
  "new" with a red box.
- **Dead ends.** No way to invite someone without e-mail ("written to the server log"; the pending row has only Cancel). No way
  to create a second organization. Promoting someone to Admin is instant, with no explanation of roles.
- **Pages about machinery.** An owner's Agents page opens on BYO posture and has no list of agents. AI models is 2,300 px of
  fieldsets with "Origin: organization ids" and "LLM posture". Internet access stacks six headings. History reads
  "#2 · superseded · 1 rules". Sign-in apps have three names and accept fake credentials.
- **Phone.** People and Pending invitations hide Status, Joined and the actions behind inner scrollbars.
- **The organization is a second world.** Four sidebars; the organization's rail replaces the workspace rail, and "home" is
  a bare list of workspaces.

The owner's decisions that apply (D-405 precedence: this brief, then D-405..D-419 and the artifact comments, then the mockup,
then the finding's fix text):

- **D-407.** Text that points to an administrator names the **role** ("Ask an admin", "Managed by your organization's admins");
  requests go to all of them. Pause/Resume replaces Freeze in the UI, organizations included.
- **D-418.** Organization owners and admins can copy an invitation link in the UI (SEC-140).
- **D-417.** One feed shape for requests and attention at organization level; Needs you reads it.
- **D-406.** Expert items (BYO, skills libraries, previews, directory sync, groups unless a provider is set) sit behind
  "Show more controls" (`Altitude`, brief 3).
- **D-410.** Policy presets sit above the organization's policy forms. Brief 18 builds the preset card; this brief leaves its
  slot (scope 8) and does not build it.

## Linked requirements

- **Artifact (source of truth):** <https://claude.ai/artifact/1YJcRA7Q84864pkim44Qnt>. Finding anchors `…#f-<key>`; the
  layer above `#above`.
- **Local reference (workers cannot open artifacts):** `/home/alex/orca/workspaces/aurora/design-ux-review-5/`, called
  `UXR5/` from here on: `UXR5/findings.md` (authoritative text), `UXR5/layer-above.md`, `UXR5/priorities-themes.md`,
  `UXR5/after/<key>.png` (dark) and `-lt.png` (light) = the visual spec, `UXR5/after/src/<key>.html` = markup (the PNG wins;
  yellow dashed notes are commentary), `UXR5/before/<key>-b<n>.png`, `UXR5/screens/<id>.png`.
- **Findings:** org-1..org-14. **Layer-above:** rec 8 (`UXR5/layer-above.md` rec 8, `…#above`: "Manage Acme" area, organization
  home as an overview), rec 7 (Needs you), rec 9 only for the naming of the area, rec 1 (altitudes), rec 15 (Pause).
- **Decisions:** D-405, D-406, D-407, D-417, D-418, D-410 (slot only). **Contracts:** brief 6 (chat-platform readiness, invitation
  link, sign-in app Test, C-1067..C-1072, SEC-140) and brief 8 (attention feed, C-1080..C-1082): read them in
  `docs/API_CONTRACTS.md` when merged. This brief adds one small contract (scope 14).
- **Docs:** `docs/14-web-dashboard.md` §Organization (`### Organization`), §Topics per level, §Instance administration only
  where it links back; `docs/05-control-plane-api.md` for invitations.

## Wave and order

- **Lane D (new)**: 17 → 18. Starts when **1, 2, 3** have merged and brief **6** (readiness, invitation link, sign-in Test) and **8**
  (attention) have merged for their items. Brief 18 starts after this one.
- Waiting items (do the rest first, and say in the report what was picked up when):

| Item | Waits for |
|---|---|
| 1 (org-1 Connect offered) and the Next line | **6** (one readiness source) |
| 4 (org-4 Copy invitation link, Resend) | **6** (link route, SEC-140; Resend only when mail works) |
| 9 (org-10 Test, "Ready (not tested)") | **6** (provider discovery Test) |
| 16 (home overview Needs you) and 1 (the asker line) | **8** |
| 14 (org-14 sentences) | the small `changes` field, scope 14 (this brief) |
| 6 (agents across workspaces) | none; fan-out over workspaces now (see there) |

## Context

All paths under `web/src/` of master c0fe7d10.

- **Shell and rails:** `components/app-shell.tsx`: `OrgNav` :200–217 (the organization's rail), `WorkspaceNav` :235, `RETURN_KEY`
  "Back to <Org>" remembering :310–360, rail composition :380–410 (`base` workspace else `orgSlug` → `OrgNav`). Switcher
  `components/nav/scope-switcher.tsx` (`ScopeSwitcher` :86, "Organization settings" row :244, "New workspace in …" :384–398,
  `canCreate` :162). Topic lists `lib/api/roles.ts` (`visibleTopics`, `ORG_TOPICS` :13), labels/icons `lib/routes.ts`
  (`TOPIC_LABEL`, `topicHref` :208, `orgRoutes`).
- **Org pages:** `app/o/[org]/layout.tsx` (just `AppShell`), `app/o/[org]/page.tsx` (home: workspace list), `components/org/org-topics.tsx`
  (`OrgAgentsPage` :55 opens on BYO, `OrgAppsPage` :83 tabs, `OrgAiModelsPage` :125, `OrgInternetAccessPage` :148, `OrgPeoplePage` :157,
  `OrgChatPlatformsPage` :180, `OrgDataPage` :208, `OrgSettingsPage` :234), `components/org/org-gate.tsx` (`OrgGate`).
  Tab bar `components/topic-page.tsx` (`useTabParam` :45, `lib/use-tab-param.ts`).
- **Org-1:** `components/channels/connector-blocked.tsx` (:54 "is added, but its connector isn't running", `setup_state ===
  "waiting_for_host"` :47), `components/connectors/` (`ConnectorsAdminView`, `available-callout.tsx`), the org page
  `OrgChatPlatformsPage` :180 and its availability query.
- **Invitations/people:** `components/org/members-view.tsx` (`MemberRow` :38, role `<select>` :77, `PendingInvitations`
  :293–340 table, `MembersView` :340, mail notice :356), `components/org/invite-dialog.tsx` (`InviteForm` :14, workspace list
  :50–80, `InviteDialog` :84), invitation hooks `useInvitations`, `useCancelInvitation` in `lib/api/queries.ts`.
- **AI models/Internet:** `components/governance/org-governance.tsx` (`aiModelsSentence` :313, `MemberAiModels` :320, `OrgAiModels`
  :395, `GovernanceForm`), `components/governance/models-tab.tsx` (fieldset per endpoint :31, "LLM posture" :14, "Key scopes" :41,
  "Origin: organization ids" :62), `components/internet/org-internet.tsx` (`OrgInternet` :144), `components/internet/internet-level.tsx`
  (`History` :87–120, `InternetLevel` :245, "History of changes" :378), `components/ui/scope-ladder.tsx`.
- **Sign-in apps:** `components/apps/oauth-clients-panel.tsx`, `components/apps/org-apps-table.tsx` (the sign-in gap rows),
  `components/admin/sign-in-gaps.tsx`; tab label "OAuth clients" `org-topics.tsx` :113.
- **Skill libraries:** `components/skills/libraries-page.tsx` :219 (plain text "Connect one under Apps & accounts first.").
- **Account types for new organization:** `components/create-org-form.tsx`, `app/page.tsx` :61 (creation on the root page; error
  `org_creation_forbidden`).
- Pieces from briefs 1–3 to use: `ResponsiveTable`, `ConfirmDialog`, `SecretOnce`, save bar, `lib/labels.ts`,
  `TechnicalDetails`, `EmptyState`, `Altitude`, the 404-in-shell pattern, `ROLE_CAN` (brief 16, `lib/roles-explain.ts`; if
  16 has not merged, create it here and 16 reuses it: say so in the report).

## Scope

Every item: build what the After PNG shows, both themes, 1256 and 390 px, with the words quoted.

### 1. Chat platforms: one readiness (org-1)

- Anchor `https://claude.ai/artifact/1YJcRA7Q84864pkim44Qnt#f-org-1`. After `UXR5/after/org-1.png`, `UXR5/after/org-1-lt.png`;
  markup `UXR5/after/src/org-1.html`; Before `UXR5/before/org-1-b1.png`, `UXR5/before/org-1-b2.png`; screens
  `UXR5/screens/A131-org-chat.png`, `UXR5/screens/A073-add-mattermost.png`, `UXR5/screens/A193-connect-chat-from-ask.png`.
- **Waits for brief 6.** The page reads the one readiness the API now gives (a running host that serves the type means
  connectable). Mattermost row: "Mattermost" / "Ready · connector running" / status **Ready** / button **Connect**. The
  "is added, but its connector isn't running" card shows only when no host serves the type; then a neutral card for admins
  ("Ask an admin": role phrase, D-407).
- When a person asked (brief 8 feed), under the row: "Carol Member asked for Mattermost. Connect it and she gets a notice."
  with **Connect Mattermost**.
- Page header "Chat platforms" / "Let people talk to agents from the chat tools Acme already uses."

### 2. Set up opens the right tab (org-2)

- Anchor `…#f-org-2`. After `UXR5/after/org-2.png`, `UXR5/after/org-2-lt.png`; markup `UXR5/after/src/org-2.html`; Before
  `UXR5/before/org-2-b1.png`; screens `UXR5/screens/A139-org-google-setup.png`.
- `TopicPage`'s tab state follows `?tab=` on **client navigation** (a `Link` that only changes the search param): in
  `useTabParam` (`lib/use-tab-param.ts`) derive the tab from `useSearchParams()` each render instead of copying it into state
  once. URL and tab always match. Test: clicking Set up on Google Drive shows the sign-in apps tab with the Google form
  ("Set up Google sign-in", "Google Drive needs a Google sign-in app. Register one once and Calendar, Drive and Gmail work for
  everyone in Acme.", Client ID, Client secret, **Save and test**, **Cancel**).
- Tab id stays `oauth-clients` in the URL (links keep working); its label is **Sign-in apps** (scope 9).

### 3. Unknown organization: a not-found inside the shell (org-3)

- Anchor `…#f-org-3`. After `UXR5/after/org-3.png`, `UXR5/after/org-3-lt.png`; markup `UXR5/after/src/org-3.html`; Before
  `UXR5/before/org-3-b1.png`; screens `UXR5/screens/A152-o-new.png`, `UXR5/screens/A142-org-app-page.png`.
- Brief 2 owns the generic "404/refused inside the shell with a link home". Here: `OrgGate`/`useResolve` failure for an unknown
  slug renders that pattern with the org copy: icon, **We couldn't find that organization**, "There is no organization called
  "new", or you don't have access to it. Check the address, or pick one of yours.", buttons **Go to Acme** (the person's
  current/default organization) and **Create an organization** (only when sign-up allows; hide otherwise). No fake rail named
  "new": the rail shows the workspace you came from (scope 5) and the switcher shows where you are. Same for `/o/<org>/w/<unknown>`.

### 4. Copy invitation link; Resend only with mail (org-4, D-418, SEC-140)

- Anchor `…#f-org-4`. After `UXR5/after/org-4.png`, `UXR5/after/org-4-lt.png`; markup `UXR5/after/src/org-4.html`; Before
  `UXR5/before/org-4-b1.png`; screens `UXR5/screens/A114-org-people.png`, `UXR5/screens/A123-invite-dialog.png`,
  `UXR5/screens/A125-invited.png`.
- **Waits for brief 6** (link route). Pending invitations (heading with a count): each row "dana@example.test" / "Invited by
  Alice Admin · Member · expires in 6 days" / **Copy invitation link** (always) / **Resend** (only when the instance's mail works:
  read the flag the API gives) / **Cancel**. After Copy: toast/inline "Invitation link copied. Send it to Dana yourself."
- The link is shown through `SecretOnce` semantics only where the API returns it once; read brief 6's contract for whether it
  is retrievable later (D-418 says retrievable by owners and admins). Never log or render it for members.
- The old notice "Email is not configured: invitation links are written to the server log." goes; a neutral line "E-mail isn't set
  up, so copy each invitation link and send it yourself." shows when mail is off.
- The created-invitation dialog state (`InviteDialog` done) also offers **Copy invitation link**.

### 5. A new organization from the switcher (org-5)

- Anchor `…#f-org-5`. After `UXR5/after/org-5.png`, `UXR5/after/org-5-lt.png`; markup `UXR5/after/src/org-5.html`; Before
  `UXR5/before/org-5-b1.png`; screens `UXR5/screens/A150-switcher.png`, `UXR5/screens/A220-root-home.png`.
- The switcher's panel gets **New organization** as the last row (plus icon), under "Your organizations" (heading in the
  mockup), shown when sign-up allows creating one (`features`/instance signup mode; the API refuses with
  `org_creation_forbidden`, so hide the row when the instance says no, and show the refusal in words if it still happens).
  It opens a dialog with `CreateOrgForm` (Name, address) and goes to the new organization's page. Brief 5/6 check that the
  create route accepts a person who already belongs to one; if not, say so in the report and stop at the UI.
- Also: a single-organization, single-workspace member with no right to create anything keeps the plain card (as `single` does).
- The row stays when a person has several organizations; the existing "All N organizations" list is unchanged.

### 6. Organization Agents opens on a list (org-6)

- Anchor `…#f-org-6`. After `UXR5/after/org-6.png`, `UXR5/after/org-6-lt.png`; markup `UXR5/after/src/org-6.html`; Before
  `UXR5/before/org-6-b1.png`; screens `UXR5/screens/A110-org-agents.png`.
- `OrgAgentsPage` tabs become **Agents · Bring your own · Skills** (Previews moves under Bring your own's page as a second
  section; BYO, Skills and Previews are Expert: `Altitude`, "n more controls hidden · Show" for the three tabs unless the person
  turned More controls on). Heading "Agents" / "Every agent in Acme, by workspace."
- Agents tab: one card; group headers "ACME TEAM", "BOB BUILDER (PERSONAL)", "CAROL MEMBER (PERSONAL)" (team workspaces first A to
  Z, then personal ones by owner name); each row avatar, name, "<purpose> · used 11 min ago" (or "never used"), and the right
  status with a dot: **Ready**, **Working**, **Asleep**, **Needs a key** (amber; "Can't answer: no AI key" reason from the
  agent status data). A row links to the agent's page in its workspace.
- Data: there is no organization-wide agents list route. Fan out `useAgents(wsId)` over `useWorkspaces(org.id)` with
  `useQueries` (cap 50 workspaces, then "Show the rest"). Say in the report if the organization-wide list should become a
  backend route (not placed by briefs 5–9).
- Owners and admins only; a member sees no Agents page here.

### 7. AI models: one summary, ids from lists (org-7)

- Anchor `…#f-org-7`. After `UXR5/after/org-7.png`, `UXR5/after/org-7-lt.png`; markup `UXR5/after/src/org-7.html`; Before
  `UXR5/before/org-7-b1.png`, `UXR5/before/org-7-b2.png`; screens `UXR5/screens/A112-org-ai.png`,
  `UXR5/screens/C061-org-ai-models.png`.
- `OrgAiModels` (owner view) opens with the member's sentence (`aiModelsSentence`): "Agents in Acme use **Company key**. People
  may add their own." Then three plain rows: **Key agents use** / "Used unless someone picks another" / "Company key";
  **People may add their own key** / "Allowed" (a switch for owners); **Who can use Company key** / "Everyone in Acme" and
  chips "Support team" with **Add team or workspace** (a picker of the organization's teams and workspaces by name: no typed
  ids).
- Below: **Endpoints (2)** collapsed (a disclosure per endpoint; inside, the display name, what it permits, key scopes in
  words) and **Advanced** collapsed (LLM posture in words, the origin settings). "Origin: organization ids / workspace ids /
  team ids" become pick lists of names (organizations, workspaces, teams). Config keys and raw ids only in
  `TechnicalDetails`. The Key scopes row never shows unchecked boxes that contradict the Limits text: derive both from the
  same `eff` value (the finding's contradiction).
- The preset slot (brief 18, D-410) is the empty first card above the summary: render `{slot}` nothing now.
- Saving follows brief 2's save bar. The confirm text is in words (brief 18 item 3 builds the sentence helper; use it when
  merged).

### 8. Internet access: Your rules and Set above you (org-8)

- Anchor `…#f-org-8`. After `UXR5/after/org-8.png`, `UXR5/after/org-8-lt.png`; markup `UXR5/after/src/org-8.html`; Before
  `UXR5/before/org-8-b1.png`; screens `UXR5/screens/A113-org-internet.png`, `UXR5/screens/C062-org-internet-access.png`.
- Heading "Internet access" / "What agents in Acme may reach." with **History**. Two groups only:
  - **Your rules** with **Add rule**: "Allow docs.example.org" / "Added by Alice Admin · 19 Nov" / **Remove**; "Ask before changes
    on other websites" / "Tightens what Aurora allows" / **Remove**.
  - **Set above you**: one line each: "Aurora's baseline" / "Common developer sites are allowed" / "Set by the instance admin";
    "Default for everything else" / "Blocked" / "Set by the instance admin". The allow-list shows once.
- Replace the stack "Limits and defaults › Limits from above › Organization allow-list › Instance baseline › Default rule ›
  Tightened below" (`InternetLevel`, `OrgInternet`); keep one Apply (save bar). The workspace-level page keeps its own layout
  until brief 18/preset work touches it; `InternetLevel` takes a prop for the two-group layout and the workspace page does
  not change.
- Preset slot above (brief 18).

### 9. One name: sign-in app; Test (org-10)

- Anchor `…#f-org-10`. After `UXR5/after/org-10.png`, `UXR5/after/org-10-lt.png`; markup `UXR5/after/src/org-10.html`; Before
  `UXR5/before/org-10-b1.png`, `UXR5/before/org-10-b2.png`; screens `UXR5/screens/A138-org-apps-tab.png`,
  `UXR5/screens/A140-org-google-added.png`, `UXR5/screens/A141-org-apps-after-google.png`.
- **Waits for brief 6** (Test via provider discovery; "Ready (not tested)").
- Everywhere "OAuth client", "Google sign-in" and "sign-in app" become **sign-in app** (tab **Sign-in apps**; page header "Sign-in
  apps" / "A sign-in app lets people connect their Google account to Aurora. Set it up once for Acme."; **Add sign-in app**).
  Brief 1 owns the word list; use its table.
- Rows: "Google sign-in app" / "Used by Calendar, Drive and Gmail" / **Ready (not tested)** / **Test**; "Atlassian sign-in app" /
  "Test passed: Atlassian answered and the client ID was accepted." / **Ready** / **Test again**. A failed Test shows
  "Test failed <when>: <reason in words>". A saved form shows **Save and test** and goes to Ready (not tested) when the Test
  cannot reach the provider (the provider must be reachable from Aurora: say so in the line).

### 10. Invite dialog: team workspaces only (org-11)

- Anchor `…#f-org-11`. After `UXR5/after/org-11.png`, `UXR5/after/org-11-lt.png`; markup `UXR5/after/src/org-11.html`; Before
  `UXR5/before/org-11-b1.png`; screens `UXR5/screens/A123-invite-dialog.png`.
- `InviteForm` lists only team workspaces (those without `personal_of`): "Invite someone to Acme"; **E-mail**; **Role** (Member /
  Admin; words "Member", "Admin" capitalised); "Also add to team workspaces" with rows (Research, Default) and their level
  selects; the line "Dana gets her own personal workspace automatically."; **Cancel**, **Create invitation**.
- Role help opens "What can each role do?" (scope 12's `ROLE_CAN`).

### 11. Connect GitHub from the skill library (org-12)

- Anchor `…#f-org-12`. After `UXR5/after/org-12.png`, `UXR5/after/org-12-lt.png`; markup `UXR5/after/src/org-12.html`; Before
  `UXR5/before/org-12-b1.png`; screens `UXR5/screens/A226-connect-repo.png`.
- `libraries-page.tsx` :219: "Connect a skill library" / "A skill library comes from a GitHub account. Acme has none connected
  yet." with button **Connect GitHub** that opens the connect journey (brief 13's journey; entry for the organization scope,
  provider GitHub) and returns to this dialog with the account chosen. The Account select reads "No account yet" until then.
  Return path uses the journey's existing `returnTo`/draft hand-off; read how brief 13 / P5-POLISH-5 pass it.

### 12. Confirm promotions with roles explained (org-9)

- Anchor `…#f-org-9`. After `UXR5/after/org-9.png`, `UXR5/after/org-9-lt.png`; markup `UXR5/after/src/org-9.html`; Before
  `UXR5/before/org-9-b1.png`; screens `UXR5/screens/A130-bob-admin.png`.
- **Owner's choice (designer's note, picked here):** ask for a change to **Admin or Owner**; lowering and Member changes apply
  at once with "Saved". The select keeps its old value until Confirm.
- Dialog "Make Bob an Admin of Acme?": person line "Bob Builder · Member now → Admin"; "Admins can, in addition to what members
  can do:" "Invite and remove people · Set up apps and chat platforms · Change internet and AI model settings"; link
  "What can each role do?" (opens the explainer fold/dialog, like the workspace page); **Cancel**, **Make Admin**. For Owner:
  "Make Bob an Owner of Acme?" with the Owner list. Roles table `ROLE_CAN` (org column) in `lib/roles-explain.ts`.
- `MembersView` gains the "What can each role do?" fold under People, as the workspace page has it.

### 13. Stacked rows on a phone (org-13)

- Anchor `…#f-org-13`. After `UXR5/after/org-13.png`, `UXR5/after/org-13-lt.png`; markup `UXR5/after/src/org-13.html`; Before
  `UXR5/before/org-13-b1.png`; screens `UXR5/screens/A203m-phone-org-people.png`.
- People and Pending invitations use `ResponsiveTable` (brief 2). Under 600 px each person is a stacked card: avatar, "Alice
  Admin", "alice@example.test", chips **Owner** and **Active**, "Joined 2 weeks ago" and the actions; an invitation: "dana@example.test"
  / "Member · expires in 6 days" / **Copy link** **Cancel**. No inner horizontal scroll at 390 px. Dates with `formatWhen`.

### 14. History reads as who changed what (org-14)

- Anchor `…#f-org-14`. After `UXR5/after/org-14.png`, `UXR5/after/org-14-lt.png`; markup `UXR5/after/src/org-14.html`; Before
  `UXR5/before/org-14-b1.png`; screens `UXR5/screens/A122-internet-history.png`.
- The dialog becomes a page section "History of internet rules" / "Each change to what agents in Acme may reach." Rows: avatar,
  "Alice added 'Block tracker'" / "19 Nov, 23:58 · current" / **Current**; "Alice added 'Allow docs.example.org'" / **Roll back to
  this**; "Bob removed 'Ask before changes'". Roll back asks (`ConfirmDialog`: "Roll back to this version? Rules added since
  stop applying now.").
- **Built by P5-REFINE-6 as C-1087** (coordinator, 2026-11-20): `PolicyGenerationSummary.changes` and `actor`. This brief
  only reads it; until P5-REFINE-6 merges the row says "<Person> published version 4" (never "#2 · superseded · 1 rules").
  Likewise org-6's list reads **C-1090** `GET /v1/orgs/{org}/agents` (P5-REFINE-6) instead of fanning out over workspaces
  once it has merged.

### 15. Manage Acme: an area, not a second rail (rec 8)

- Reference `https://claude.ai/artifact/1YJcRA7Q84864pkim44Qnt#above` rec 8 (`UXR5/layer-above.md` rec 8). No dedicated mockup;
  follow the pattern of brief 11's Settings area (left sub-nav on desktop, a tab/select strip on phones) and the visual kit.
- On `/o/<org>/…` (not `/w/…`) the rail stays the **workspace rail of the last workspace the person was in** (remembered per
  tab like `RETURN_KEY`; else their personal workspace; else the first). Remove `OrgNav` from the rail. A persistent link at the
  top of the area: **← Back to <workspace name>**.
- Inside the page area, a **Manage Acme** header (the only place the organization is named) with a sub-nav: Overview · Agents ·
  People · Apps & accounts · Chat platforms · AI models · Internet access · Data · Settings (role-filtered with
  `visibleTopics("org", …)`; a member keeps the three member pages: Overview, AI models, Internet access). Entered from the
  switcher's **Manage Acme** row (was "Organization settings") and the account menu. Page-header "Acme" badges disappear from
  workspace pages when the person has one organization and one workspace.
- Deep links from mail, gateway errors and every old address keep working (C-960 redirects stay); active state follows the path.
- The tour's org stops (if any) follow the area. The switcher's "Pause Acme"/"Resume" naming follows brief 1 (Freeze retired
  from the UI).

### 16. The organization home is an overview (rec 8, rec 7)

- Reference `…#above` rec 8 and rec 7. `app/o/[org]/page.tsx` becomes Manage Acme › **Overview**:
  - **Needs you** (the organization's attention feed, brief 8: requests for apps, sign-in apps, AI keys, chat platforms that an
    org admin can resolve; pending invitations; broken shared accounts). Same row shape as everywhere. Until brief 8 merges:
    today's `SetupRequestItem` rows.
  - **People**: count, pending invitations count, **Invite** button.
  - **Workspaces**: the list (team first, personal below), **New workspace**.
  - **Agents** across workspaces: the last five with a status dot and **All agents**.
  - **AI models** one line (scope 7 sentence) and **Internet access** one line (reach sentence), each linking to its page.
- Members see Workspaces and the two lines only.

## Out of scope

- Policy presets card and confirm sentences (brief 18); Administration pages (18).
- The vocabulary table (brief 1), primitives (2), altitude registry (3). Backend of readiness, invitation link and Test (6),
  attention (8).
- The organization-wide agents list route (note only).

## Acceptance criteria

1. Every item matches its After PNG (dark) and `-lt.png` (light) at 1256 px and 390 px; no horizontal scroll at 390.
2. Chat platforms offers Connect whenever a host serves the type, and the same readiness is shown in Administration.
3. Set up on Google Drive shows the sign-in apps tab and form; `?tab=` and the visible tab always agree after client navigation.
4. `/o/new` and an unknown workspace render the not-found card inside the shell with Go to Acme / Create an organization.
5. A pending invitation has Copy invitation link always and Resend only with mail; members never get the link.
6. The switcher has New organization when sign-up allows it.
7. Agents opens on the grouped list; BYO, Skills and Previews are behind More controls; the status words match.
8. AI models opens with the summary sentence; endpoints and advanced are collapsed; no typed ids; no unchecked scope boxes that
   contradict the text.
9. Internet access shows exactly two groups; the allow-list appears once.
10. "Sign-in app" is the only name; Test and "Ready (not tested)" work (after brief 6).
11. The invite dialog lists team workspaces only. Promotion to Admin or Owner asks with what the role can do.
12. People and invitations are stacked at 390 px with every field and action visible.
13. History rows are sentences with Roll back (after the `changes` field).
14. The Manage Acme area replaces the organization rail, with a Back link and old links intact; Overview shows Needs you,
    people, workspaces, agents and the two lines.
15. No snake_case, UUID or raw id outside `TechnicalDetails`/`<code>` on these pages (label guard).
16. Web gates (targeted): typecheck, lint on touched files, the vitest and mocked Playwright specs below with axe.

## Tests

Targeted only; GitHub CI is the full gate. Heavy commands through `/home/alex/orca/workspaces/aurora/heavy.sh <lane> <command>`;
never `localhost:4000` or the aurora-dev cluster.

- **Vitest:** connector card states from readiness; tab follows `?tab=` on search-param change; the not-found card variants;
  invitation row actions per mail flag and role; switcher New organization visibility; agents grouping and status words;
  models summary sentence and the pick lists; internet two-group layout; sign-in app status lines; invite workspace filter;
  role-change confirm (Admin and Owner ask, Member does not); history sentence builder with and without `changes`; org
  sub-nav visibility per role; overview sections.
- **Playwright (mocked)** at 1280 and 390 px in both themes, axe, no console errors: Set up on Google Drive; `/o/new`; copy
  invitation link; New organization; org Agents; AI models; Internet access; promote Bob; phone People; Manage Acme navigation and
  Back link.
- **Rust:** `cargo test -p aurora-api -p aurora-db` targeted for the `changes` field (through `heavy.sh`).
- Update specs that read "OAuth clients", "Organization settings", "is added, but its connector isn't running", the org rail.

## Docs to update

- `docs/14-web-dashboard.md` §Organization, §Topics per level (the Manage area replaces the organization rail), §Onboarding
  (switcher); `docs/API_CONTRACTS.md` and `docs/05-control-plane-api.md` for `changes`; `docs/PROGRESS.md`/`docs/FEATURES.md`
  rows, current at every commit.

## Suggested agent/model and reason

Sonnet (`nextjs-developer`) for the web items; `rust-developer` for the small `changes` field. The Manage area touches the shell
and routing, so review it closely.

## Completion report

Fill in when done; P5-REFINE-20 reads it.

- **Items done** (number, finding keys, commit):
- **Items waiting for a backend brief** (which, what is missing, when picked up):
- **Deviations from the mockup or this brief and why:**
- **Screenshots compared** (key, theme, width, path of the new screenshot):
- **Tests run** (commands and counts):
- **Open questions:**
