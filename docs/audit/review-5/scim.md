# REVIEW-5 findings — scim (saved by the coordinator from the reviewer's hand-back)

## SCIM-1 (high, security) — OIDC role-claim sync re-creates a membership for a SCIM-removed/suspended person
- `services/aurora-api/src/domains/oidc/roles.rs:350`, `:495-520` (`apply_org` → `db::org_insert` at :518, no `scim::gates::admission_gate`). JIT gate at `oidc/service.rs:812`, invitation gate at `invitations/service.rs:293`.
- Spec: SEC-73 "a suspended or removed link keeps JIT, invitations and sign-in from re-admitting the person"; C-474.
- Fix: gate `apply_org` before `org_insert`; `gates::audit_denied` path "idp_sync"; skip the grant. Test: SCIM provision → DELETE → sign in with mapped claim → no membership + denied audit row.

## SCIM-2 (medium, concurrency, PLAUSIBLE) — admission gate reads race SCIM deprovisioning outside any lock
- `scim/gates.rs:44` plain SELECT; `oidc/service.rs:812` (no lock); `invitations/service.rs:293` (gate before `lock_org` at :308); `scim/resource_users.rs:1238-1262` (`delete_user` unlocked read, no org lock); `:888`.
- Fix: `groups::lock_org` in `apply_jit`/`accept_in_tx` before the gate and in `delete_user`/suspend branch; or FOR KEY SHARE on the link in the gate.

## SCIM-3 (high, security) — SCIM suspend claims a human suspension as its own; IdP reactivation lifts it
- `scim/resource_users.rs:397-402`, `:453` (adopt active link over an already-suspended membership, flag false), `:1003-1005` (suspend branch writes `suspended_by_scim = true` unconditionally), `:1029-1039` (reactivation trusts flag), `deprovision.rs:66`. Correct guard in `move_link` `:1188`.
- Spec: C-473, docs/04 §SCIM Reactivation, SEC-73 "a reactivation never … overrides a human suspension".
- Fix: `by_scim = member.status != "suspended"` in the suspend branch.

## SCIM-4 (high, spec-defect) — POST/PUT adopts an existing signed-in account by SCIM-asserted e-mail as an active member; contradicts SEC-73
- `scim/resource_users.rs:354-360`, `:374-389`, `:441-457`; `scim/management.rs:118-133` (`clean_domains`, no ownership proof; instance allow-list optional).
- docs/04 §SCIM Users step 2(b)/3 (docs/04:815) vs SEC-73 "linked … only through the OIDC-verified e-mail (never SCIM-asserted data)".
- Scenario: org owner enables SCIM for `gmail.com`, POSTs victim@gmail.com → victim's account becomes an active member of that org.
- Owner decision: (a) defer membership until the person's own OIDC sign-in, or (b) verified domains.

## SCIM-5 (medium) — `exclusive` mode bypassed by default-org placement and role-claim sync
- `placement/service.rs:118-126`; `oidc/roles.rs:495-520`; `docs/operator/scim-provisioning.md:23-24`; docs/04 §883.

## SCIM-6 (medium) — unfiltered list / displayName filter capped at 10,000 rows; `totalResults` wrong
- `scim/resource_users.rs:526-528`, `:544-547`, `:557-560`; `repo/scim.rs:249-268`.

## SCIM-7 (medium, concurrency) — user PATCH merges over a snapshot read outside the transaction (lost updates; stale userName for D-167)
- `scim/resource_users.rs:647-651`, `:654-673`, `:691-703`, `:979-991`, lock at `:888`.

## SCIM-8 (low) — adopt with active:false skips `cancel_invitations`
- `scim/resource_users.rs:403-420` vs `:1025-1027`, `:1178-1180`. C-474.

## SCIM-9 (low) — body read/parsed before authentication; bearer-less not charged; malformed {id} → non-SCIM 400
- `scim/protocol.rs:374-390`, `:404-412`; `scim/rate_limit.rs:103-108`. C-470 identical 401.

## SCIM-10 (low) — `emails[type eq "..."].value` ignores the filter and overwrites the primary e-mail
- `scim/resource_users.rs:756-775`.

## SCIM-11 (medium, test-gap) — SEC-73 "proven" without tests for SCIM-1/2/3/7/8; no counter-outage test for SCIM; no list > 10k; no concurrent user PATCH

## SCIM-12 (low, PLAUSIBLE, spec) — owner link attributes PATCHable (`:893-904`, `:930-946`) vs SEC-72 "an owner is never mutated by SCIM"

## Not covered
- OIDC bind retry loop (`oidc/service.rs:581-605`); put_settings domain removal / exclusive toggle effects; group PATCH/PUT FK race vs DELETE; groups list total; replay fixtures; whether each FIX-030..070 fixes its claim.
