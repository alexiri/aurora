# P4-ONB-2: Onboarding — instance setup wizard, Welcome page and guide, tour (owner, 2026-11-09)

## Why
A new user today lands on Overview with a three-item checklist and must work out alone:
- what an LLM key is;
- that the agent needs access to it;
- that the agent may need a model.

The owner's own first production session failed exactly there (FIX-019..FIX-021). The goal is to get people from "just signed in" to "my agent answered" with no help, and to explain the few concepts on the way. The instance admin likewise needs a guided first setup.

## Spec (source of truth; read, don't re-decide)
- `docs/14-web-dashboard.md` **§Onboarding** (all three parts), plus the Overview note, Account ("Show the tour again") and Admin › Setup.
- `docs/05-control-plane-api.md`: `GET/PATCH /v1/me/onboarding` and `GET /v1/admin/setup`.
- D-141; `14 §Visual design` (tokens, shared primitives and page patterns, both themes, no horizontal scroll at 390 px).

## What it looks like
1. **Instance setup wizard** (instance admins).
   - Shown until a placement is saved; Admin › Setup afterwards.
   - A stepper: roles (shows your token's claim and what it mapped to), where new users land (the P4-ONB-1 form), LLM access (endpoints, `default_model` warning, add an org key and make it a starter grant), agents can run (runners, image, gateway), optional apps, try it.
   - Live status per step.
   - Config-only things are shown read-only, with the Helm value to change.
2. **Welcome page** at a user's first sign-in.
   - Three cards with real names: your workspace and org, your agent and how it works (own computer, gateway, approvals), your role.
   - Then **"Get Daneel talking"**: LLM key → give access → model (only if needed) → say hello.
   - Steps tick from real state (model readiness, grants, a conversation with a reply). Steps already done (an org key from starter grants) appear collapsed with what provides them.
   - Members who cannot do a step see who can.
   - "Skip for now" leads to Overview, where the same guide stays until done or dismissed.
3. **Tour**: "Show me around" coach marks over the navigation (6–8 stops, keyboard, reduced motion, 390 px opens the drawer). Replayable from the account menu.
4. **Teaching empty states** on every list touched by the journey. Errors (`no_model_key`, `no_default_model`, missing grant) link to the guide step that fixes them.

## Basic shape (implementer decides details)
- `user_onboarding` (or a user-prefs jsonb) for welcome/tour/dismissed state.
- Guide completion is **computed**, never stored.
- `GET /v1/admin/setup` aggregates existing health data (runners, gateways, endpoints) with reason codes.
- One guide component replaces the Overview checklist.
- Copy reviewed with the house style in `14` (short sentences, no jargon, real names).

## Acceptance
- Vitest for each guide state (key provided by the org, key missing, not granted, no model, done; member vs owner).
- Playwright journeys:
  - a fresh instance: admin wizard to a working chat;
  - a new user with a starter grant: Welcome → first reply with no LLM step;
  - a new user without one: the guide walks key → access → model → reply.
- Screenshots at 1280 and 390 px, both themes; axe clean; tour keyboard test.
- Usual web gates.

## Depends on
FIX-019 (readiness route), FIX-021 (access step, deep links), P4-ONB-1 (placement, Welcome target). P4-IDP-1 for the roles step (show "not configured" without it).
