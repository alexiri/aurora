# P5-UX-7: Welcome, and one summary of what an agent can do (owner, 2026-11-15)

## Goal

A new person's first page should explain Aurora in the same few sentences the product uses everywhere,
and tell the truth about their agent:
- Welcome says "The gateway … asks you before risky actions." In a fresh workspace nothing asks first. A
  design review found that the promise and the facts disagreed.
- The guide has a step "Give Daneel access to it", an extra confirmation even though Daneel is the only
  agent.
- "Connect an app" led nowhere; it should open the journey for this agent.

This task rewrites Welcome around the five sentences of the model, shortens the guide to two steps, and
builds the **agent's summary**: a few sentences computed from real state, shown on Welcome and on the
agent's Access tab (D-350).

## Linked requirements

- **Design** (D-354): the canvas “Aurora — agents, accounts and access”, https://claude.ai/artifact/3UwmUgrhh6MLCsfqdhTFmQ, boards Workspace › Welcome and Overview; Agent › Access (the summary) and Agents (list). Read each board's note; where a board and the spec disagree, the spec wins.
- D-350, D-326, D-343; contracts **C-927, C-986 (`agent.ts`)**.
- Normative: `14 §Information architecture` "The model in five sentences" and the vocabulary,
  `14 §Agents, accounts and access` §8 and §9, `14 §Onboarding` §2 (superseded where §9 says so),
  `07 §Evaluation` (what the sentences must be true to).

## Wave and order

- **Wave 3.** Starts after P5-UX-3 (the journey), P5-UX-4 (accounts and access), P5-IA-2
  (`summary/internet.ts`), P5-UX-14 (the health line; the Overview block it adds) and P5-UX-2 (C-988: a
  key added with access).
- P5-UX-5 follows it and adds **Add people** to Welcome's links.

## Context

- **Welcome.** `web/src/components/onboarding/welcome-view.tsx` (three cards with fixed text) and
  `getting-started.tsx` (the guide: steps computed from state in `lib/onboarding/guide.ts`;
  `OptionalNext` ~351–371).
- **Guide state.** Model readiness (C-500), the agent's access to a key, conversations. Progress is kept
  per user on the server (`user_onboarding`).
- **AI-key form.** `components/credentials/llm-form.tsx`; since P5-UX-4 it saves the key with the ticked
  agents in one request (C-988).
- **Summaries.** P5-IA-2's `web/src/lib/summary/internet.ts` gives the sentences about websites.
  P5-UX-4's accounts list (`GET …/accounts?agent_id=`) gives the agent's accounts and access.

## Scope

1. **`summary/agent.ts` (C-986).** A pure function from one agent's accounts and the workspace's
   `internet.ts` sentences to ordered sentences. The sentences are exactly the table of `14` §8, in its
   order; do not reword them:
   - the accounts it can use ("Daneel can read GitHub and Linear."; more than three: "GitHub, Linear and
     3 more");
   - Ask before changes ("It asks you before changing anything in Linear."), with always-allowed changes
     ("…, except creating issues."; more than two: "except 3 kinds of change you always allow");
   - Change without asking ("It can change AWS without asking.");
   - Custom ("In Notion it has custom access.");
   - an account that needs attention ("It can't use AWS right now: the sign-in expired.", from P5-UX-14's
     health line in lower case);
   - the reach sentence from `internet.ts` with the subject "It" ("It can reach any website."), and its
     ask sentence only when a rule asks ("A rule asks you first on 2 websites.");
   - for an agent with no accounts: "Daneel can't use any app yet. It can reach any website."

   Each condition is a row of a table the unit tests walk. Every sentence must be true under
   `07 §Evaluation`.
2. **Place the summary.**
   - Welcome's third card, titled "<Agent> is your agent".
   - The top of the agent's Access tab, above the accounts list.
   - The workspace Overview, once Welcome is no longer the landing page.
3. **Welcome's page and cards (C-927).** Welcome stays at `…/welcome` and is where a person lands in a
   workspace until they finish or skip the guide; after that the workspace opens on Overview. The title
   is "Welcome to <workspace>", or "Welcome, <name>" in the person's own personal workspace. Three
   cards, with real names, from the five sentences:
   1. where you are: "This is **<workspace>**, in **<org>**. <Agent> works here, on its own computer."
   2. how it stays safe: "Everything <Agent> sends goes through Aurora's gateway. It holds the keys,
      applies the access you choose, and puts anything that needs your say in Approvals."
   3. the agent's summary.

   A person who is not an owner sees their role in one sentence and `WhoCan` where a step is not theirs.
4. **The guide (C-927).** Two steps, ticking themselves from real state:
   1. **AI key.** Done when the agent can already use one ("Your organization provides an AI key").
      Otherwise the form, with "<Agent> can use this key" ticked. A model picker appears only when the
      key's endpoint has no default model.
   2. **Say hello.** Opens Chat with a suggested first message.

   A step "Give <Agent> access to <key>" appears only when a key exists that the agent cannot use.
   `lib/onboarding/guide.ts` and its "n of m" line follow.
5. **Next, when you are ready.** **Give <Agent> an app** (the journey for this agent), Write instructions,
   Set up a schedule. Each link comes from `routes.ts`, and a test checks that each resolves.
6. **Words.** Welcome, the guide and the tour sentences use the vocabulary: AI key, account, access,
   Approvals.
7. **Tests.**
   - Vitest: the summary table (a new agent; read-only apps; one app asking; one asking with one and
     with three always-allowed changes; one changing without asking; custom; a broken account; a long
     list; a workspace where websites are restricted; a rule that asks); the Welcome title in a personal
     and in a shared workspace; the guide's
     states (organization key; no key; a key the agent cannot use; no default model); the cards for an
     owner and a member.
   - Playwright:
     - a new person in an organization that provides a key sees one step left and a summary that says
       the agent can't use any app yet;
     - a new person with no key adds one and goes straight to Say hello;
     - after giving the agent an app with Ask before changes, Welcome and the Access tab both say that
       it asks before changing anything there.
   - Axe and narrow-route suites for Welcome and the Access tab, both themes.

## Constraints / file ownership

- Owns: `lib/summary/agent.ts`; `welcome-view.tsx` (title included); `getting-started.tsx`;
  `lib/onboarding/guide.ts`; the summary block of the Access tab and of the workspace Overview.
- Uses, and does not change, the accounts list, the journey and `internet.ts`.
- No API change.

## Acceptance criteria

1. Welcome explains Aurora with the five sentences and real names, and promises nothing the summary does
   not show.
2. A new person with an organization key has one step to a working chat; without a key, adding it is the
   only step before Say hello.
3. The same summary sentences appear on Welcome and on the agent's Access tab, and they are true for
   every row of the test table.
4. Web gates; axe and narrow-route suites, both themes.

## Suggested agent/model and reason

Sonnet (`nextjs-developer`). The summary's truthfulness is the risky part; the rest is page content.

## Completion report (lane p5-apps)

Commits: part 1 `2a4c1e55` (summary), part 2 `c1d53e73` (Welcome and guide), part 3 `8a13c866` (Overview). FIX-164 (`89a7149b`) later made the summary read the member route for every viewer.

- **`lib/summary/agent.ts` (C-986).** `agentSummary({ agent, agentId, accounts, internet })`, a pure function over the accounts list read for the agent (`given !== false`) and `internet.ts`'s sentences, in the order of the `14` §8 table. Truth rules, written in the file header and walked by `tests/agent-summary.test.ts` (23 rows): `read`/`ask`/`act`/`use` all allow reads, so those accounts are "can read" (websites with a token included, an AI key is not listed); `custom` is said as "In <App> it has custom access." and never as "can read"; `ask` asks before every change except the always-allowed ones (one sentence for the plain ones, one per account with exceptions, "except 3 kinds of change you always allow" from three); `act` is "can change … without asking"; an account that needs attention is not usable, so it is said only as "It can't use <App> right now: <health clause>." and in no other sentence (and a lone broken account is not "no app yet"); a turned-off account is not usable; two accounts of one app are told apart by their names. The ask sentence is said only when a rule asks. With the Internet access unreadable (`null`) nothing is said about websites.
- **Placement.** `AgentSummary` (components/agents) on Welcome's third card ("<Agent> is your agent"), at the top of the Access tab ("What <Agent> can do"), and in each agent card of the Overview. One `<p>` of sentences.
- **Welcome (C-927).** Title "Welcome to <workspace>" or "Welcome, <name>" in the viewer's personal workspace (`useSession` memberships); three cards: where you are, how it stays safe (the five sentences with real names; Approvals links to the page), the summary. A non-owner reads their role in one sentence under the title. The old promise "asks you before risky actions" is gone. Welcome stays the landing page until the guide is finished or skipped: opening the page no longer marks it seen; **Skip for now**, **Show me around** and a completed guide do.
- **Guide (C-927).** Two steps: *AI key* (done when the agent can use one; otherwise the form with "<Agent> can use this key" ticked; when the key works but its endpoint has no default model, the same step holds the model picker) and *Say hello*. "Give <Agent> access to <key>" exists only while a key exists that the agent cannot use, and then replaces the key step. `step=model` links land on the key step. "n of m" follows. Words: AI key, Approvals.
- **Next.** **Give <Agent> an app** opens the connect journey at its App step (`…?tab=access&connect=`, appended because `url()` drops an empty value); each link still resolves (who-can test).
- **Overview.** `NeedsYou` (components/overview) is one card: the approvals (for people who may decide) and P5-UX-14's accounts that need attention, each with its fix or who can fix it; `OverviewAccountsAttention` is gone. The board's page sentence ("Daneel is working on one conversation…") is not built (not in the spec).
- **Fix found on the way.** Saving an AI key with its access (C-988) did not refresh the guide's model readiness, so the guide stayed on the key step; `useCreateSecret` now invalidates `["model-readiness"]` (`lib/api/secrets.ts`).
- **Spec over board.** The board's Welcome card 3 adds "Nothing asks you first until you give it an app with Ask before changes." (a promise the summary table does not have) and its card 5 says "You can only tighten them"; neither is built. The board's five-sentence list on Welcome is the three cards of C-927.
- **Tests.** vitest: `agent-summary.test.ts`, `welcome.test.tsx` (title personal/shared, cards, role sentence, a member's summary, summary on the Access tab), `onboarding-guide.test.ts` and `onboarding.test.tsx` (the two-step states, organization key, no key, key the agent cannot use, no default model, member), `who-can.test.tsx` (the Next link). Playwright (mocked) `welcome-summary.spec.ts`: an organization key leaves one step and the agent can't use any app yet; no key → add one → Say hello; Give Daneel an app with Ask before changes → Welcome and the Access tab both say it asks; each in both themes at 1280 and 390 px with axe and no horizontal scroll. `first-run.spec.ts` and `onboarding-journeys.spec.ts` (real stack) were updated for "of 2" and the new step titles but **not run** here (they need the live stack).
