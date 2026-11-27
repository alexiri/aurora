# Navigation review — findings, proposals and the owner's decisions

Source of truth for the P5-NAV briefs. Each proposal has Before screenshots (`before/`, today's Aurora) and an After mockup (`after/<key>.html`, open it in a browser, `?light` for the light theme; rendered as `after/<key>.png` and `after/<key>-light.png`).

The owner chose **nav-2a** (agent settings as tabs, on every agent page) and **nav-3** (places, the place picker, Manage as a place, with nav-1, nav-4 and nav-5 as its parts), and accepted every proposed decision (2026-11-27). nav-2b and nav-2c are **not** chosen; they stay here only for the record.

Captured on master 937396e4 · throwaway kind cluster aurora-skm (never aurora-dev) · 1256 px and 390 px · dark · 2026-11-27.

Every menu in Aurora was opened as Bob Builder (owner of his workspace, with “Show more controls” on, as in the owner's screenshots) and as Alice Admin (owner of Acme and instance administrator): the workspace rail, the agent page and its Settings, Manage Acme, Administration, Your account and the phone drawer. The pages with two left menus are the agent's Settings and every Manage <org> page; Administration and Your account already show one.

## In short

- **Two left menus where one would do.** An agent's Settings adds a 208 px menu beside the 256 px rail; Memory and Skills then add a list, so the page has four columns and the content gets about 700 of 1256 px. Manage Acme adds a 192 px menu the same way.
- **The rail doesn't say where you are.** On Manage Acme the rail keeps showing a workspace's entries, none of them active, beside the organization's own Agents, Skills, People, Apps & accounts, Internet access and Settings: six names twice, meaning different things.
- **Aurora already has the better pattern.** Administration and Your account replace the rail with their own menu and a way back. Manage and the agent's Settings should work the same way, or move their sections into tabs.
- **Every place has its own way back.** “‹ Chat” (a button), “← Back to your workspace”, “← Back to Acme”, and nothing at all on Your account.
- **Phones get the worst of it.** An agent's Settings shows the header tabs, “‹ Chat”, the title and seven section pills wrapping onto two rows before any content; Manage Acme becomes a native select.

## Findings

- **N-1** (Gets in the way) Every agent Settings page shows two left menus: the rail and a 208 px Settings menu. On Memory and Skills a list column follows, so the page has four columns and the memory or skill gets the last ~450 px. Before: `before/N04-agent-settings-instructions.png`, `before/N05-agent-settings-memory.png`, `before/N06-agent-settings-skills.png`
- **N-2** (Gets in the way) Settings is a button beside the Chat · Automations · Computer tabs, but it opens a different frame with its own “‹ Chat” back button, an h2 and its own menu. The agent header reads as two navigations at once. Before: `before/N03-agent-chat.png`, `before/N04-agent-settings-instructions.png`
- **N-3** (Gets in the way) On a phone an agent Settings page stacks the header tabs, “‹ Chat”, the page title and seven section pills that wrap onto two rows, before any content. Before: `before/P02-phone-agent-settings.png`, `before/P04-phone-agent-instructions.png`
- **N-10** (Gets in the way) The agent header moves between sections: on Chat and Computer the tabs sit top right beside Settings, on Automations the page is narrower and they drop under the name, and on Settings a “‹ Chat” button and a page title push the content down. Before: `before/N03-agent-chat.png`, `before/N21-agent-automations.png`, `before/N07-agent-computer.png`, `before/N05-agent-settings-memory.png`
- **N-4** (Bug) Manage Acme keeps the workspace's rail (Overview, Agents, Skills, Apps & accounts, Internet access, Approvals, Activity, People, Settings, none of them active) beside its own menu (Overview, Agents, Skills, People, Apps & accounts, Chat platforms, AI models, Internet access, Data, Settings). Six names appear in both columns and mean different things. Before: `before/N12-manage-overview.png`, `before/N13-manage-agents.png`, `before/N15-manage-people.png`
- **N-5** (Gets in the way) Three places, two models: Administration and Your account replace the rail with their own menu; Manage Acme and the agent's Settings add a second one. The ways back differ: “‹ Chat”, “← Back to your workspace”, “← Back to Acme”, and none on Your account. Before: `before/N17-admin-overview.png`, `before/N20-account.png`, `before/N13-manage-agents.png`
- **N-6** (Gets in the way) Manage Acme can only be found as a plain row in the switcher's popover (no icon), or through “All n in Acme”; the rail has no entry for it, while Administration has one at the rail's foot. Before: `before/N11-switcher-open.png`, `before/N10-alice-ws-overview.png`
- **N-7** (Polish) Your account keeps the workspace switcher at the top of its rail and has no way back to where you were. Before: `before/N20-account.png`
- **N-8** (Polish) On a phone Manage Acme's menu is a native select above the page, and the drawer still lists the workspace's entries. Before: `before/P03-phone-manage.png`, `before/P05-phone-drawer-manage.png`
- **N-9** (Polish) Tabs come in two styles without a rule: segmented for the agent's sections and Approvals, underline for page topics; admin pages carry up to six tabs although `14` says at most four. Before: `before/N18-admin-agents.png`, `before/N08-ws-settings.png`

## The agent page (round 1)

### nav-2a — Agent settings as tabs (recommended)

**Today:** Settings is a button that opens a second left menu.

**Proposal:** The agent header gets one row of tabs, **Chat · Automations · Computer · Settings**; Settings is a tab like the others. Its sections are a second, quieter row under it (Instructions · Apps & access · Memory · Skills · Tests · Other agents · Manage). The rail stays the workspace's, and the settings page gets the full content width.

<ul><li>Applies to every agent page: see “nav-2a, page by page” below for Chat, Automations, Computer and the phone.</li></ul>

**Fixes:** N-1, N-2, N-10

**Before:** `before/N04-agent-settings-instructions.png`, `before/N05-agent-settings-memory.png`

**After:** `after/nav-2a.html`, `after/nav-2a.png`, `after/nav-2a-light.png`

### nav-2b — Alternative: the rail drills into the agent *(not chosen)*

**Today:** Same problem as above.

**Proposal:** The rail becomes Daneel's menu, the way Manage and Administration would work: “‹ Agents”, Daneel's card, Chat · Automations · Computer, then a **Settings** group with its sections. One column; the cost is that the workspace's other pages are one step further away while you are in an agent.

**Fixes:** N-1, N-2

**Before:** `before/N05-agent-settings-memory.png`

**After:** `after/nav-2b.html`, `after/nav-2b.png`, `after/nav-2b-light.png`

### nav-2c — Alternative: keep the menu, collapse the rail *(not chosen)*

**Today:** Same problem as above.

**Proposal:** The smallest change: while a second menu is open, the rail shrinks to icons (64 px) with tooltips. The Settings menu stays as it is. It buys back ~190 px but keeps two menus and three ways of navigating.

**Fixes:** N-1

**Before:** `before/N05-agent-settings-memory.png`

**After:** `after/nav-2c.html`, `after/nav-2c.png`, `after/nav-2c-light.png`


## nav-2a, page by page

What nav-2a changes on every agent page: one header that never moves (avatar, name, status, the agent's “…” menu) and one row of tabs, **Chat · Automations · Computer · Settings**. A second, quieter row appears only under Computer and Settings, which have sub-pages. Every section uses the full content width. The Settings button and the “‹ Chat” button go. Addresses stay the same (`?tab=memory` still opens Memory), so links, bookmarks and tests that use them keep working. Two details to settle when building it: the second row adds one line under Computer and Settings, so their content starts lower than Chat's (the header itself stays put); and on Chat the agent's “…” in the header sits above the conversation's own “…”, so the header one should read as the agent's (its name or “Agent” beside it). On a phone Chat still opens on the conversation list, as today; the mockup shows a conversation opened from it.

### nav-2a-chat — Chat

**Today:** The tabs sit top right next to a separate Settings button.

**Proposal:** The same header as every other section, with Settings as the fourth tab. The conversation list and the status rail stay; they are part of the page's content, framed as panels, not menus.

**Fixes:** N-2, N-10

**Before:** `before/N03-agent-chat.png`

**After:** `after/nav-2a-chat.html`, `after/nav-2a-chat.png`, `after/nav-2a-chat-light.png`

### nav-2a-automations — Automations

**Today:** The page is narrower than the others, so its tabs drop under the agent's name.

**Proposal:** Full width like every section. The header stays where it is; under the tabs, the page's own title, its filter (All · Schedules · Webhooks · Chats) and the list.

**Fixes:** N-10

**Before:** `before/N21-agent-automations.png`

**After:** `after/nav-2a-automations.html`, `after/nav-2a-automations.png`, `after/nav-2a-automations-light.png`

### nav-2a-computer — Computer

**Today:** Computer has its own strip of sub-pages under a header laid out differently from Settings.

**Proposal:** Computer's sub-pages (Files · Terminal · Connect · Previews · Details) use the same quiet second row as Settings' sections, so both sections with sub-pages look alike.

**Fixes:** N-10

**Before:** `before/N07-agent-computer.png`

**After:** `after/nav-2a-computer.html`, `after/nav-2a-computer.png`, `after/nav-2a-computer-light.png`

### nav-2a-phone — Every section on a phone

**Today:** On a phone the header tabs, “‹ Chat”, the title and seven pills on two rows come before any content.

**Proposal:** A compact header (back to Agents, avatar, name, status, “…”), the four tabs as one underline row, and the second row of Computer or Settings as one line of pills that scrolls sideways. Nothing wraps.

**Fixes:** N-3

**Before:** `before/P02-phone-agent-settings.png`, `before/P04-phone-agent-instructions.png`

**After:** `after/nav-2a-phone.html`, `after/nav-2a-phone.png`, `after/nav-2a-phone-light.png`


## Manage, Administration, account (round 1)

### nav-1 — Manage is a place: the rail becomes its menu

**Today:** Manage Acme shows a workspace's rail beside its own menu.

**Proposal:** Do what Administration already does. Choosing **Manage Acme** turns the rail into the organization's menu: a way back (“‹ Your workspace”), a scope card “Acme · Organization”, then Overview · Workspaces · Agents · Skills · People · Apps & accounts · Chat platforms · AI models · Internet access · Data · Settings (Workspaces is new: today the organization's workspaces are listed on its Overview). One column, and the page gets the width back.

**Fixes:** N-4, N-5

**Before:** `before/N13-manage-agents.png`, `before/N12-manage-overview.png`

**After:** `after/nav-1.html`, `after/nav-1.png`, `after/nav-1-light.png`

### nav-3 — Where you are, and how to get to Manage

**Today:** Manage Acme hides in the switcher as a plain row.

**Proposal:** The switcher becomes a scope picker with three labelled groups: **Workspaces**, **Organization** (Manage Acme, with what it holds and its badge) and **Administration** for instance admins. The rail's foot gets a visible **Manage Acme** entry for owners and admins, beside Administration.

**Fixes:** N-6

**Before:** `before/N11-switcher-open.png`, `before/N10-alice-ws-overview.png`

**After:** `after/nav-3.html`, `after/nav-3.png`, `after/nav-3-light.png`

### nav-4 — One header pattern for every place

**Today:** Each place has its own back link, and Your account has none.

**Proposal:** The rail's top always has the same two parts: a way back (except in a workspace, the home) and a scope card saying where you are: “Your workspace · Acme”, “Acme · Organization”, “Administration · This Aurora”, “Bob Builder · Your account” (and “Daneel · Asleep” if 2b is chosen).

**Fixes:** N-5, N-7

**Before:** `before/N05-agent-settings-memory.png`, `before/N13-manage-agents.png`, `before/N17-admin-overview.png`, `before/N20-account.png`

**After:** `after/nav-4.html`, `after/nav-4.png`, `after/nav-4-light.png`

### nav-5 — On a phone

**Today:** Phones stack two navigations and wrap seven pills; Manage is a native select.

**Proposal:** The drawer shows the current place's menu with its way back. An agent's Settings is the fourth header tab, and its sections are one row of pills that scrolls sideways instead of wrapping. Manage pages keep only their title and their own tabs.

**Fixes:** N-3, N-8

**Before:** `before/P02-phone-agent-settings.png`, `before/P03-phone-manage.png`, `before/P05-phone-drawer-manage.png`

**After:** `after/nav-5.html`, `after/nav-5.png`, `after/nav-5-light.png`


## nav-3 in depth: places, the picker and Manage

nav-3 rests on one idea: Aurora has **places**. A workspace is where people and agents work; its organization (Manage Acme) holds what every workspace shares; Administration holds this Aurora's settings; Your account is yours. The rail always shows one place, its top always says which, and the place picker takes you to the others.

### nav-3a — The place picker, for three different people

**Today:** Bob, who can only use his workspace, gets the same popover as Alice, and “Manage Acme” is a plain row among workspaces.

**Proposal:** The picker lists only places you can enter, labelled by kind: **Workspaces in Acme**, **Organization** (Manage Acme, for owners and admins), **Administration** (instance admins), **Other organizations**. Search appears when there are many places and searches across organizations.

<table><thead><tr><th>Picker group</th><th>What it lists</th><th>Who sees it</th></tr></thead><tbody>
<tr><td>Search</td><td>Every place you can enter, across organizations</td><td>Anyone with more than 6 places</td></tr>
<tr><td>Workspaces in Acme</td><td>The 5 most recent, your role in each, ✓ on the current one; “All n workspaces” (owners and admins: Manage › Workspaces; members: the full list in the picker)</td><td>Everyone in Acme</td></tr>
<tr><td>Organization</td><td>Manage Acme: what it holds, your role, a count of what needs you</td><td>Acme's owners and admins, instance admins</td></tr>
<tr><td>Administration</td><td>Settings for everyone on this Aurora, with its count</td><td>Instance admins</td></tr>
<tr><td>Other organizations</td><td>Up to 3, your role in each; choosing one opens your last workspace there (or its Manage when you have none)</td><td>People in more than one organization</td></tr>
<tr><td>New</td><td>New workspace in Acme, New organization</td><td>People allowed to create them</td></tr>
</tbody></table>
<ul><li>A person with one workspace and nothing else to open gets the card without a menu, as today.</li>
<li>The rail's foot repeats <b>Manage Acme</b> (owners and admins) above <b>Administration</b> (instance admins), each with its count.</li>
<li>Search (Ctrl K) also finds places: “Manage Acme”, “Administration”, any workspace by name.</li></ul>

**Fixes:** N-6

**Before:** `before/N22-switcher-bob.png`, `before/N11-switcher-open.png`

**After:** `after/nav-3a.html`, `after/nav-3a.png`, `after/nav-3a-light.png`

### nav-3b — Inside Manage Acme

**Today:** Manage's menu is a flat list of ten entries in a second column, beside a workspace's rail.

**Proposal:** The rail is the organization's menu, grouped the way Administration's is: Overview · Workspaces · People, then **What agents can use** (AI models · Apps & accounts · Chat platforms · Internet access · Skills), then **Oversight** (Agents · Data), then Settings. The Overview says what needs the owner, with links.

<table><thead><tr><th>Entry</th><th>Today</th><th>Tabs inside</th></tr></thead><tbody>
<tr><td>Overview</td><td>Overview (also lists workspaces)</td><td>— (what needs you, summary)</td></tr>
<tr><td>Workspaces</td><td><i>new</i>: moved out of Overview</td><td>— (list, New workspace)</td></tr>
<tr><td>People</td><td>People</td><td>Members · Groups · Directory sync</td></tr>
<tr><td colspan="3"><b>What agents can use</b></td></tr>
<tr><td>AI models</td><td>AI models</td><td>—</td></tr>
<tr><td>Apps &amp; accounts</td><td>Apps &amp; accounts</td><td>Accounts · Apps · Sign-in apps · Self-hosted apps</td></tr>
<tr><td>Chat platforms</td><td>Chat platforms</td><td>—</td></tr>
<tr><td>Internet access</td><td>Internet access</td><td>—</td></tr>
<tr><td>Skills</td><td>Skills</td><td>Skills · Libraries</td></tr>
<tr><td colspan="3"><b>Oversight</b></td></tr>
<tr><td>Agents</td><td>Agents</td><td>Agents · Bring your own · Templates</td></tr>
<tr><td>Data</td><td>Data</td><td>Retention · Storage · Audit log · Usage</td></tr>
<tr><td>Settings</td><td>Settings</td><td>General · API keys · Danger zone</td></tr>
</tbody></table>
<ul><li>The back line returns to the workspace <i>and page</i> you came from; opened from a link with no history, it returns to your last workspace in Acme; with no workspace in Acme it is hidden.</li>
<li>Page titles name the scope: “Agents in Acme”, “People in Acme”, so the same word in a workspace and in Manage never reads as the same page.</li>
<li>The count on Manage Acme covers only what an owner or admin can act on (library updates to review, broken organization accounts, people waiting to join).</li></ul>

**Fixes:** N-4

**Before:** `before/N12-manage-overview.png`, `before/N15-manage-people.png`

**After:** `after/nav-3b.html`, `after/nav-3b.png`, `after/nav-3b-light.png`

### nav-3c — How the places connect

**Today:** Each place is entered and left in its own way.

**Proposal:** Four places, one rule: the place card at the top of the rail says where you are and, outside a workspace, the way back. Manage and Administration are also one click from the rail's foot for the people who may enter them.

**Fixes:** N-5, N-7

**Before:** `before/N13-manage-agents.png`, `before/N17-admin-overview.png`, `before/N20-account.png`

**After:** `after/nav-3c.html`, `after/nav-3c.png`, `after/nav-3c-light.png`

### nav-3d — Places on a phone

**Today:** On a phone Manage's menu is a native select, and the drawer shows the workspace.

**Proposal:** The top bar shows the place card in short; tapping it opens the picker as a sheet. The drawer shows the current place's menu with its back line.

**Fixes:** N-8

**Before:** `before/P05-phone-drawer-manage.png`, `before/P03-phone-manage.png`

**After:** `after/nav-3d.html`, `after/nav-3d.png`, `after/nav-3d-light.png`

## The owner's decisions (all accepted, 2026-11-27)

- **Can members see Manage?** Decided: no. Members can't change anything there; what Acme provides (AI models, apps) is already shown where they use it. The alternative is a read-only “About Acme” page.
- **Workspaces as their own entry?** Decided: yes. Today the list hides in Manage's Overview, the most common thing an owner looks for there.
- **Is Agents “oversight”?** Decided: yes. In Manage, Agents is the list of every agent in the organization, to pause or review, not a place to work with one.
- **Manage in the rail's foot as well as in the picker?** Decided: yes, for owners and admins only, the way Administration already is for instance admins.
- **Administration in the picker too?** Decided: yes. The picker is the one list of every place you can enter; the rail's foot is the shortcut. Your account stays in the user menu, not in the picker.
- **Where does the back line go?** Decided: to the workspace and page you came from, not the workspace's Overview.

## Also

- **One rule for tabs.** Segmented controls switch a view of the same data (Active · Archived, Checklist · Health); underline tabs move between pages of one topic. At most five; more go into a “More” menu or become their own page. Before: `before/N18-admin-agents.png`, `before/N08-ws-settings.png`
- **Update `14` when this is decided.** The shell section of `docs/14-web-dashboard.md` still describes two stacked selectors, a level badge on every page header and the opposite tab styles; it should describe the chosen model, so the next screen follows it.

## All screens

- `before/N01-ws-overview-default.png`
- `before/N02-ws-overview.png`
- `before/N03-agent-chat.png`
- `before/N04-agent-settings-instructions.png`
- `before/N05-agent-settings-memory.png`
- `before/N06-agent-settings-skills.png`
- `before/N07-agent-computer.png`
- `before/N08-ws-settings.png`
- `before/N09-ws-agents.png`
- `before/N10-alice-ws-overview.png`
- `before/N11-switcher-open.png`
- `before/N12-manage-overview.png`
- `before/N13-manage-agents.png`
- `before/N14-manage-skills.png`
- `before/N15-manage-people.png`
- `before/N16-manage-settings.png`
- `before/N17-admin-overview.png`
- `before/N18-admin-agents.png`
- `before/N20-account.png`
- `before/N21-agent-automations.png`
- `before/N22-switcher-bob.png`
- `before/P01-phone-drawer.png`
- `before/P02-phone-agent-settings.png`
- `before/P03-phone-manage.png`
- `before/P04-phone-agent-instructions.png`
- `before/P05-phone-drawer-manage.png`
