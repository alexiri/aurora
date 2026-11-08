# P2-FU-1: Phase-2 spec gaps left by merged lanes (phase 2)

Five small, independent gaps. Each has its own tests. Contract numbers: **C-195..C-199**. Decision: **D-96**.

## (a) Attachment quota notices by e-mail (FU-P2-015b)
`02` limit `ATTACH_QUOTA_NOTICE` (phase 2, RFE-015): notices at **80 %** and **100 %** of the org attachment quota (`10 §Attachments`).
- Send them through the RFE-011a e-mail channel that P2-011A built (the approval notifier/mailer: reuse its transport, templates and dev mail sink; don't build a second mailer).
- Recipients: the org's owners and admins.
- Send once per threshold crossing per quota period. A drop below the threshold and a new crossing notifies again. Keep the de-duplication state in the DB, in an additive migration if no existing table fits.
- No attachment names or contents in the mail; only the org, usage, quota and a link to the usage page.
- Tests: crossing 80 % sends one mail to each owner/admin, crossing 100 % sends one more, repeats don't send, and the mail body contains no attachment data.

## (b) "Expired by retention" in the transcript (FU-P2-015a)
`14` (Conversations, line ~325): "expired attachments render as 'expired by retention'".
- Find how the API marks an attachment purged by retention (RFE-015, `purged_at` or equivalent).
- Make the transcript component render that state as a chip that uses the shared primitives (`14 §Visual design`, tokens only, both themes), never as a broken link or a download button.
- Tests: a vitest for the component, plus an API test if the DTO needs the flag (it may already have it).

## (c) Caller's current address on the Connect tab (FU-P2-025b, owner Q7 = yes)
`14` (agent BYO settings, line ~378) wants "the caller's current address as a hint".
- Add **one read-only field**: the client address **as the API sees it** behind `server.trusted_proxies`. Put it on `GET /v1/me`, or on the agent's BYO settings response if that is cleaner, and say which in the contract, C-195.
- Nullable and documented. No new route.
- The UI shows it next to the source-list editor with a note that the gateway may see a different address (NAT, load balancer, PROXY protocol).
- Tests:
  - API: a direct peer, a trusted proxy with `X-Forwarded-For`, and an untrusted peer sending `X-Forwarded-For`, which must be ignored;
  - web: the hint renders.

## (d) Missing indexes (FUP-M7)
- `agents.avatar_public_key` (public avatar lookup scans) and `attachments.blob_key`. One additive migration with `CREATE INDEX` (check whether the table sizes call for `CONCURRENTLY`; follow the repo's migration conventions).
- Update `docs/03` so the schema guard (`scripts/extract-schema.py --check`) stays green.

## (e) `presence.provision` carries `agent.avatar_url` (FUP-M7)
`13 §4.2` (op table) says the `presence.provision` payload includes the agent's avatar URL. `services/aurora-api/src/domains/channels/presences.rs::enqueue_provision` omits it.
- Add it, in the same shape `set_icon` and the config feed already use.
- Tests: the provision payload contains it, and the connector fake or SDK test accepts it.

## (f) App icon fallback (owner report, 2026-11-08)
`web/src/components/apps/apps-tab.tsx` renders `<img src="/v1/apps/{id}/icon">` with no fallback, so a failed or missing icon shows the browser's broken-image glyph.
- Add a shared `AppIcon` primitive in `web/src/components/ui`, modelled on `AgentAvatar`. It renders the icon and falls back on `onError`, or when the app has no icon, to a monogram tile: the app name's first letter on a token background, in both themes.
- In the dark theme, use the package's dark icon when the API says it has one: `assets/icon-dark.png`, `has_dark_icon` and the dark variant of the icon route in `services/aurora-api/src/domains/apps/assets.rs`. Check how the route selects the variant. Near-black brand glyphs (GitHub, Notion) are invisible on dark surfaces otherwise.
- Use it everywhere an app or package icon appears: the apps tab, connections, the approvals/activity rows if they show app icons, and the catalogue.
- Tests: a vitest for the fallback.

## Constraints
- Additive migrations only.
- The new field in (c) must not reveal anything beyond the caller's own address.
- Dashboard changes follow `14 §Visual design`; the production CSP forbids inline `style=`.
- **Owns:**
  - the API mailer use for (a) and its migration;
  - the transcript attachment component for (b);
  - `/v1/me` or the BYO settings DTO plus the web BYO settings section for (c);
  - one migration for (d) plus `docs/03`;
  - `presences.rs` for (e);
  - the docs and contracts named above.
- Not the gateway (P2-FU-2 runs in parallel there).

## Acceptance criteria
- [ ] The tests named in each item.
- [ ] `cargo test -p aurora-api -- --test-threads=8` green; fmt and clippy `-D warnings`; `UPDATE_OPENAPI=1` regeneration plus `pnpm gen:api` committed.
- [ ] `scripts/extract-schema.py --check` green.
- [ ] web `pnpm typecheck`, `lint`, `test` and `build` green.
- [ ] `bash scripts/guards/run-all.sh`.
