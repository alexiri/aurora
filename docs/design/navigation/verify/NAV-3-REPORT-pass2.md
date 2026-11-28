# Places verification, pass 2 (P5-NAV-5): after FIX-282 and FIX-283

Date: 2026-11-28. Reviewer lane, no product code changed.

## Result

**Clean: all 16 rows are Match, so P5-NAV-5 is done.** C-1 to C-5 and the small items are fixed on the live stack, the
neighbours show no change for the worse, and CI is green on `890d11e8`. The owner has accepted no extra differences,
and none are needed.

## Environment

- Commit: `890d11e8` (master with FIX-282 and FIX-283). `aurora-mem5` (:4530) redeployed with
  `kind-dev.sh up --llm-mock --ssh` through `heavy.sh`, then `kubectl rollout restart` of every deployment, since `up`
  again left the old Pods running. `aurora-dev` and `aurora-skm` untouched.
- The same people as in pass 1: Alice (instance admin, owner of Acme), Bob (one workspace), Carol (member of Acme, owner
  of Globex), Dave (member of both), and Alice as emergency reader on Bob's workspace.
- Data for C-1: the Anthropic skills updates still waiting from pass 1, Bob's two app requests, and two organization
  accounts I added to Acme: a website secret and an outgoing webhook. I marked them broken **by editing the throwaway
  database** (the secret: last use rejected; the webhook: sign-in required).
- Playwright, 1256 and 390 px, both themes on the screens that changed and on every Manage page; walk-throughs mostly
  dark. Heights 783 and 640 px for the rail. Axe, console, `/v1` 4xx and sideways overflow recorded.
- Through the dashboard or API I also: created and deleted a workspace as Dave; turned **Members can create
  workspaces** off and on again; rejected Acme's waiting library update; deleted both added accounts; opened and ended
  emergency grants.

## The five items

**C-1, Needs you and the counts: fixed.**
- Alice's count was 5: "Anthropic skills has an update to review", the two requests, "The GitHub bot token account for
  Acme stopped working" and "The Release hook account for Acme needs someone to sign in again". The rows end in
  **Review** or **Failed**, as in `nav-3b.png`.
- The rail's badges: Overview 5, **Apps & accounts 4, Skills 1**. The rail's foot and the picker show 5.
- Only the organization's update is counted; the updates waiting on a workspace's and an agent's skills aren't.
- Review opens Skills. Fix opens Apps & accounts › Accounts. Sign in again opens that page with the reconnect dialog.
- Deleting the two accounts and then rejecting the update took the count 5 → 4 → 3 → 2, and the badges followed.
- Who sees it: Bob and Dave get 403 on Acme's feed (Bob 404 on Globex) and no count. Carol, who owns Globex, gets only
  Globex's empty feed and a 403 on Acme's.
- The title says "stopped working" where the mockup says "failed to refresh its token". FIX-282 explains why: the
  server can't tell that a refresh was the cause. I count that as Match.

**C-2, "Your workspace" on the back line: fixed.** Bob and Carol, going from their own workspace to Your account (and
Bob by an agent's `?from=` link), read "‹ Your workspace". Carol, coming from Bob's, still reads "‹ Bob's workspace".

**C-3, back line after a grant: fixed.** From Manage, Administration and Your account, Alice's back line showed Bob's
workspace while the grant was open. After ending it, the next page of that place showed "‹ Default" (her last
workspace) and it opened Default. A fresh load after the end does the same. I ended the grants; I didn't wait for one
to expire.

**C-4, short window: fixed.**
- At 783 and 640 px high the active entry is in view on all 11 Manage entries (and an app's page), seven
  Administration pages (down to the last entry), three Account pages and two workspace pages, also after clicking from
  the rail's foot and from the menu. It is in view in the phone drawer at 390×640 too.
- If the person scrolls the menu up, it stays there (checked after 8 s).

**C-5, New workspace for members: fixed.**
- With the setting on, Bob, Carol, Dave and Alice are offered **New workspace in Acme** (Dave also on a phone, and
  **New workspace in Globex** while in Globex). Bob, with one workspace, now has it too.
- Dave created a workspace from the picker, landed in it as its owner, and saw it in the picker.
- With the setting off, Bob, Carol and Dave lose the entry for Acme, `/v1/me` says `false`, and creating through the
  API is refused (403). Dave still gets it for Globex, whose setting is on. Alice, an owner, keeps it.

**Small items: fixed.**
- A reason ending in "." or "?" gets no second full stop.
- Searching "globex" now lists Globex's workspaces (and Manage Globex); "acme" does the same for Acme.
- Manage › Internet access makes no 403 request for Alice. Its new sentence, "From the workspaces you can read.", only
  appears with the list of recently blocked hosts. This cluster has none, so it is covered by FIX-283's tests, not
  seen.

## Registry

| Row | Pass 1 | Pass 2 |
|---|---|---|
| Back line | Minor (C-2, C-3) | **Match** |
| Manage menu | Minor (C-4) | **Match** |
| Overview | Needs changes (C-1) | **Match** |
| Picker | Needs changes (C-5) | **Match** |
| One place, Place card, Workspaces, Titles, Members, Search in picker, Rail foot, Ctrl K, Phone, N-4..N-8, Tests, Rules | Match | **Match** (re-walked) |

## Neighbours

I repeated the pass 1 walk: all 11 Manage pages (card, back line, menu, titles, tabs, axe), Bob's redirects, Bob's Ctrl
K, the pickers of Alice, Bob, Carol and Dave, keyboard and search, recent-first order, Alice's Ctrl K, Carol in Globex,
the back line through Manage, Administration and Account, direct links and old addresses, the Workspaces page, the
phone drawers, and the reader's picker (Bob's workspace listed only while on its pages). Nothing got worse.

Sideways scrolling is as in pass 1: inside the tab strips and tables at 390 px, never the page.

## Tests and CI

- `e2e/places.spec.ts` and `e2e/place-picker.spec.ts`, mocked, through `test.sh` on `890d11e8`: **34 passed**, FIX-283's
  short-window test among them.
- The test diff since pass 1 adds no skip, `fixme`, `only` or `ignore`.
- CI run 46100344241 for `890d11e8`: **success** in every job that ran (two release and pull-request jobs were
  skipped). `web`: 4,139 unit tests passed; the Playwright runs passed 2,189 (4 skipped) and 13.

## Other observations (not blocking, not counted)

- `GET /v1/admin/setup` still answers 404 on some Administration visits, as in pass 1. The pages work.

## Still open for the owner

Whether Manage's count should include people waiting to join (PROGRESS, open questions). Today the invitations row
stays uncounted.

## Not checked live

- The back line hidden when there is no workspace, the card without a menu (everyone on this cluster may create an
  organization), "Other organizations" opening Manage when the person has no workspace there, and Directory sync (no
  SCIM): as in pass 1.
- A paused skill library, skill libraries switched off, and what an API key is told: FIX-282's tests cover these.

## Cluster

**Torn down** with `kind-dev.sh down`. The three named cache volumes (`p5-memory-5-kind-dev-target`,
`-cargo-registry`, `-rustup`) are kept, and nothing was pruned. One unnamed volume went: the deleted kind node's own
`/var` volume (`9769ba31…`), which held the cluster and its database. `kind delete cluster` removes it with the node.
The other 11 volumes are unchanged. `aurora-skm` is still there.
