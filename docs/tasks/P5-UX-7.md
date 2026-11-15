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

- D-350, D-326, D-343; contracts **C-927, C-986 (`agent.ts`)**.
- Normative: `14 §Information architecture` "The model in five sentences" and the vocabulary,
  `14 §Agents, accounts and access` §8 and §9, `14 §Onboarding` §2 (superseded where §9 says so),
  `07 §Evaluation` (what the sentences must be true to).

## Wave and order

- **Wave 3.** Starts after P5-UX-3 (the journey), P5-UX-4 (accounts and access), P5-IA-2
  (`summary/internet.ts`) and P5-UX-2 (C-988: a key added with access).
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
   `internet.ts` sentences to ordered sentences:
   - which apps and websites the agent can read ("Daneel can read GitHub and Linear.");
   - where it asks before changes ("It asks you before changing anything in Linear.") and where it may
     change things without asking;
   - an account that needs attention ("It can't use AWS right now: the sign-in expired.");
   - the website sentences ("It can reach any website. Nothing else asks you first.");
   - for an agent with no accounts: "Daneel can't use any app yet."

   Each condition is a row of a table the unit tests walk. Long lists are shortened ("GitHub, Linear and
   3 more"). Every sentence must be true under `07 §Evaluation`.
2. **Place the summary.**
   - Welcome's third card, titled "<Agent> is your agent".
   - The top of the agent's Access tab, above the accounts list.
3. **Welcome's cards (C-927).** Three cards, with real names, from the five sentences:
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
5. **Next, when you are ready.** Connect an app (the journey for this agent), Write instructions, Set up a
   schedule. Each link comes from `routes.ts`, and a test checks that each resolves.
6. **Words.** Welcome, the guide and the tour sentences use the vocabulary: AI key, account, access,
   Approvals.
7. **Tests.**
   - Vitest: the summary table (a new agent; read-only apps; one app asking; one changing without
     asking; a broken account; a long list; a workspace where websites are restricted); the guide's
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

- Owns: `lib/summary/agent.ts`; `welcome-view.tsx`; `getting-started.tsx`; `lib/onboarding/guide.ts`; the
  summary block of the Access tab.
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
