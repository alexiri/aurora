# P4-ONB-1: New-user placement — default org, shared workspaces, a workspace per user (owner, 2026-11-09)

## Why
Under `open` sign-up every new user gets their own org and is its owner (the owner's production sign-in landed in "Alex Iribarren's Org"). On a shared instance the instance admin decides where people land. Example: everyone joins org *Users*, and `bobj` gets workspace *bobj* with the default agent in it.

## Spec (source of truth; read, don't re-decide)
- `docs/04-identity-tenancy.md` **§New-user placement** and the `open` row of §Sign-up.
- `docs/05-control-plane-api.md`: `GET/PUT /v1/admin/placement` and `…/preview`.
- `docs/14-web-dashboard.md`: Admin › **Sign-up & placement**.
- `docs/17-security.md` **SEC-79**; D-140. Owner: **dashboard only, no config key**.

## What it looks like
- **Admin › Sign-up & placement**, a form with:
  - mode (`shared_org` / `personal_org` / `none`);
  - default org (pick, or create one by name) and the new users' role in it;
  - shared workspaces with roles;
  - personal workspace on/off, its role, and a **name template** with a live preview ("bobj" → `bobj`; "Ada Lovelace" with `{name}` → "Ada Lovelace", slug `ada-lovelace`);
  - **starter grants**: pick org LLM keys, secrets or connections that every new personal workspace's agent receives;
  - a warning while today's per-user-org behaviour is active, and the list of personal orgs created before.
- **bobj's first sign-in**: he joins *Users* and the shared workspaces, *bobj* is created with Daneel already holding the org key, and he lands on its Welcome page (P4-ONB-2).
- **Later sign-ins** find his workspace by his user id, even after renames.
- Turning personal workspaces on later gives existing users theirs at their next sign-in.
- On a **fresh** instance under `open`, non-admins see "This instance is being set up — try again shortly" until an admin saves the placement.

## Basic shape (implementer decides details)
- A singleton placement row (or an instance-settings table).
- `workspaces.personal_owner_user_id`, unique per (org, user).
- Applied in the sign-in transaction when the user row is created, and idempotently at later sign-ins when the personal workspace is missing.
- Reuses the workspace creation transaction (`04` steps 2–4).
- Starter grants go through the shared grant function from FIX-020, with the system as actor.
- Template variables come only from validated claims. Slugify; numbered suffix on collision; fallback chain, then `user-<short id>`.
- Record tables in `03 §Planned changes`. Contract numbers from the coordinator.

## Acceptance
- SEC-79 tests (in `17`), proven, with the coverage row updated.
- The permission matrix covers the new routes.
- e2e with the dev IdP: two users sign in → one org, two personal workspaces, each with a default agent holding the starter grant; a second sign-in of the same user creates nothing; a colliding name gets `-2`.
- Dashboard page: vitest + Playwright (390 px, both themes, axe).
- Usual gates.

## Depends on
FIX-020 merged (grant function). Pairs with P4-IDP-1 (precedence) and P4-ONB-2 (the setup wizard hosts this form as step 2).
