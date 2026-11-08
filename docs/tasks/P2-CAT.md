# P2-CAT-1..6: Built-in app catalogue packages (phase 2)

## Goal
Add the first built-in package set (`19 §Phase 2`), one lane per group below. Every lane runs **after P2-CAT-0 has merged** and follows its framework exactly: `plugins/builtin/README.md`, the GitLab reference package, and the checks every package must pass (`21 §Phase 2 — App catalogue`).

## Common rules (every lane)
- Read `docs/09-app-plugins.md` §3 (manifest contract), §5 (mechanisms), §8 (hooks, only if your group needs one), §12 (CLI and fixture format), and `plugins/builtin/README.md`.
- Each package: `plugin.yaml`, `guide.md` (step-by-step client registration plus the generated permissions table), icons, `tests/*.yaml` covering every host entry, every tool (at least one positive and one negative match), every summary template and the token lifecycle against **recorded HTTP with scrubbed credentials**. No live calls in tests.
- Narrowest scopes by default; write scopes marked. Tools are read-only unless the API operation writes. Write tools get clamped summaries.
- Use the vendor's official API documentation for hosts, paths, scopes and token endpoints, and cite each `about.api_docs` / `api_spec` URL. Never guess an endpoint: if the docs are unclear, leave the tool out and report it.
- Don't change the manifest contract or gateway mechanisms. If a package needs something the contract lacks, stop and ask through Orca `ask`.
- Self-hostable products (Jira/Confluence Data Center, Sentry self-hosted, …): once P2-INST has merged, declare `instances` (`09 §3.4 Instance-bound hosts`, D-74) so one package serves the SaaS host and registered self-hosted instances. Lanes that start before P2-INST ship the SaaS host only and list the self-hosted follow-up in their report.
- You own only `plugins/builtin/<your packages>/**`. Shared files (guards, CI, the README) are P2-CAT-0's; report needed changes instead of making them.
- Acceptance: `aurora plugin validate`, `aurora plugin test` and the P2-CAT-0 checks pass for every package of your group; the dev stack lists them; guards pass.

## Lanes

### P2-CAT-1 — GitHub (OAuth) and GitHub App (hooks)
- `github`: OAuth app (authorization code), REST v3 + GraphQL (read tools; issues/PR writes marked), git over HTTPS.
- `github-app`: GitHub App installation tokens minted from the app's private key (the mint mechanism of `09 §5`), webhook signature verification through a hook (`09 §8`). This is the hooks example of the catalogue.

### P2-CAT-2 — Jira and Confluence (shared OAuth client)
- One package with two apps sharing one Atlassian OAuth 2.0 (3LO) client (`09 §3.3 oauth_clients`); cloud id discovery as connection metadata. Jira REST v3 (issues, search, transitions marked write) and Confluence REST v2 (pages, spaces; writes marked).

### P2-CAT-3 — Google Workspace
- One package: Gmail, Calendar, Drive under one Google OAuth client with incremental, narrow scopes (read-only by default; send/modify marked). Drive download summaries by MIME type through a hook, as `19` states.

### P2-CAT-4 — Microsoft 365
- One package over Microsoft Graph (Entra ID OAuth; tenant as connection metadata): mail, calendar, OneDrive/SharePoint files; read-only by default, writes marked.

### P2-CAT-5 — AWS keys and AWS role
- `aws-keys`: SigV4 request signing with an access key pair (the signing mechanism of `09 §5`), region/service derived from the host.
- `aws-role`: assume-role minting of short-lived credentials (mint mechanism), then SigV4. Start with a small set of services (S3, STS, a read-only EC2 subset) and say which.

### P2-CAT-6 — Sentry, Notion, Linear
- `sentry` (auth token; org/project slugs as metadata), `notion` (OAuth public integration, `Notion-Version` header), `linear` (OAuth, GraphQL single endpoint with per-operation tool classification as the GraphQL fixture shows).

### PostgreSQL (not a lane yet)
The `postgresql` package needs the gateway database lane (`P2-DBL`). It is added by P2-DBL or right after it, not here.

## Suggested agent/model
Sonnet per lane, own worktree. Lanes 1–6 touch disjoint directories and can run in parallel.
