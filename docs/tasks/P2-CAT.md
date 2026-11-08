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

## Prerequisite: P2-CAT-H — hook-capable built-ins
P2-CAT-0 built the framework for packages **without hooks**: `scripts/plugins/check-builtin.sh` and `scripts/plugins/build-builtin.sh` refuse hooks, and the image build has no `wasm32-wasip2` / `cargo component` toolchain. Lanes that need a hook wait for **P2-CAT-H**, which adds:
- the hook source layout for built-ins (`09 §2`, `§8`; build with `aurora plugin build` as `§12` describes), compiled in the image build (`plugin-archives` stage: install the pinned toolchain) and in CI's built-in checks;
- `check-builtin.sh` support for hooks: build them, run hook fixtures through the embedded hook-host runtime against recorded HTTP (`§12 aurora plugin test`), and keep the coverage rules;
- one small example hook in a fixture or in the first package that needs it.
Needed by: CAT-1 (`github-app` webhook verification), CAT-3 (Drive MIME summaries), CAT-5b (`aws-role` assume-role minting through a `token.mint` hook).

## Lanes

### P2-CAT-1 — GitHub (OAuth) and GitHub App (hooks; `github-app` after P2-CAT-H)
- `github`: OAuth app (authorization code), REST v3 + GraphQL (read tools; issues/PR writes marked), git over HTTPS.
- `github-app`: GitHub App installation tokens minted from the app's private key (the mint mechanism of `09 §5`), webhook signature verification through a hook (`09 §8`). This is the hooks example of the catalogue.
- **Coordinator correction (2026-11-08, D-91), replacing the webhook item above:** `09 §8.2` has no inbound-webhook hook point, and agent webhooks are core (`10 §Webhooks`: `hmac_sha256` over the raw body). `github-app` therefore uses `token.kind: hook` with a **`token.mint`** hook:
  - It signs an RS256 app JWT in the guest with a pure-Rust signer. The JWT has `iat = now − 60`, `exp ≤ now + 600` and `iss = <app id>`; `now` comes from the host's `now-unix-seconds`.
  - It calls `POST https://{host}/app/installations/{installation_id}/access_tokens` and returns the token. The expiry is GitHub's `expires_at`, capped by `max_lifetime_secs`.
  - Egress is declared for `api.github.com`, and for the instance host on GHES (`/api/v3`).
  - Connect is a form with these fields: app id, installation id (metadata), and the private key PEM (a credential field). It never goes into metadata, logs or summaries.
  - An optional `connect.exchange` mints once to validate the form. Its error must not echo the key.
  - Webhooks are out of scope. If GitHub's `X-Hub-Signature-256` matches core's `hmac_sha256` verification, the guide may say so in one line; otherwise report it as a follow-up.
- Both packages declare GHES through `instances` (D-74; `gitlab` is the reference). The GHES API base is `https://{instance}/api/v3` and GraphQL is `https://{instance}/api/graphql`; git over HTTPS uses the instance host. `github.com`/`api.github.com` is the default host.

### P2-CAT-2 — Jira and Confluence (shared OAuth client)
- One package with two apps sharing one Atlassian OAuth 2.0 (3LO) client (`09 §3.3 oauth_clients`); cloud id discovery as connection metadata. Jira REST v3 (issues, search, transitions marked write) and Confluence REST v2 (pages, spaces; writes marked).

### P2-CAT-3 — Google Workspace (after P2-CAT-H for the Drive hook)
- One package: Gmail, Calendar, Drive under one Google OAuth client with incremental, narrow scopes (read-only by default; send/modify marked). Drive download summaries by MIME type through a hook, as `19` states.

### P2-CAT-4 — Microsoft 365
- One package over Microsoft Graph (Entra ID OAuth; tenant as connection metadata): mail, calendar, OneDrive/SharePoint files; read-only by default, writes marked.

### P2-CAT-5 — AWS keys (and P2-CAT-5b — AWS role, after P2-CAT-H)
- `aws-keys`: SigV4 request signing with an access key pair (the signing mechanism of `09 §5`). `aws_sigv4` takes the service as a literal and the region from a template, so one host entry per service with the region from connection metadata (a host/metadata region mismatch fails at AWS; documented). S3, STS GetCallerIdentity, a read-only EC2 subset.
- `aws-role` (**P2-CAT-5b, after P2-CAT-H**): there is no declarative mint mechanism, so assume-role minting is a `token.mint` hook (SigV4-signed `sts:AssumeRole`, egress `sts.amazonaws.com`), then SigV4 with the session credentials.

### P2-CAT-6 — Sentry, Notion, Linear
- `sentry` (auth token; org/project slugs as metadata), `notion` (OAuth public integration, `Notion-Version` header), `linear` (OAuth, GraphQL single endpoint with per-operation tool classification as the GraphQL fixture shows).

### PostgreSQL (not a lane yet)
The `postgresql` package needs the gateway database lane (`P2-DBL`). It is added by P2-DBL or right after it, not here.

## Suggested agent/model
Sonnet per lane, own worktree. Lanes 1–6 touch disjoint directories and can run in parallel.
