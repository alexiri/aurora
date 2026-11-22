# P5-ASRC: Addon sources — third-party app-package repositories (owner, 2026-11-10)

Admins can register third-party **addon sources** (Home-Assistant-style): a git repository that
publishes an addon-repo manifest listing available Aurora app packages, who signs them, and where
the signed `.aurora-plugin` archives live. Aurora reads the manifest, offers the packages for
installation, polls for updates on a configurable schedule and highlights pending updates.

This task is the platform side: source registration, manifest ingestion, the update poll, the admin
UI and the public format documentation. The first consumer (the CERN repo with two addons) is
P5-CERN-ADDONS.

## Shape

A new **package source** `addon` beside `builtin`, `directory` and `upload` (`09 §10`). Packages
from a source are ordinary packages in every other respect: same format, same linter, same
lifecycle (`pending_approval` → `active`), same connect flows, grants and gateway injection. Nothing
about "addon" changes what an installed package can do — only where it came from and how updates
arrive.

### The addon-repo manifest (`aurora-addons.yaml`)

A small YAML file at a stable HTTPS URL (raw file of the repo; the admin may paste either the repo
URL or the direct manifest URL — Aurora normalizes and stores the resolved manifest URL). Contract
versioned (`api: 1`), strict parse:

- `repo`: name, description, homepage, contact (https URL or mailto).
- `publisher`: the package publisher id all addons of this repo use.
- `signing_key`: the **ed25519 public key** (base64) packages are signed with, plus a short
  key description. This is the trust anchor of the source.
- `addons[]`: one entry per addon — `id`, `name`, `description`, `versions`: a map of semver →
  `package` (https URL of the `.aurora-plugin` archive), `sha256`, `size_bytes`, `min_core`.

The addon manifests themselves do not change: `plugin.yaml` stays the phase-0 contract 1 manifest
(`package.version`, `publisher`, permissions, apps, tools…). The format documentation states which
existing fields matter for distribution (version, publisher matching the repo's, icon, setup guide)
and that the repo manifest carries the repo-level info. Additive manifest schema changes are out of
scope.

### Registration (instance admin)

Admin → **Addon sources** → Add: paste the URL → Aurora fetches the manifest and shows a review
screen: repo name/description/homepage/contact, publisher, **signing-key fingerprint** (prominent),
the addon list with versions, and how many packages are new to this instance. Confirming:

1. creates the source row (url, manifest digest, key fingerprint, `update_interval`),
2. creates a **`trusted_keys` row** for the publisher's key (`09 §2` signing trust) — one audited
   step, so verification composes with the existing machinery,
3. ingests the listed packages: each archive is downloaded, size- and digest-checked, verified
   against the source's key, linted and stored `pending_approval` with
   `source = "addon", source_id = <row>`.

Permissions of addon packages always require the ordinary instance-admin approval (third-party
permissions are never auto-activated on first sight, unlike `builtin`).

### Polling and updates

- Instance setting **`plugins.addon_sources.update_interval`** with exactly the owner's values:
  `6h` | `12h` | `1d` | `1w` | `2w` | `1mo` | `never`. Default `1d`. `never` = manual only
  ("Check now" per source). One setting for all sources (per-source override: not in v1).
- On each poll (background job, per-source lease like P4-GSL's): re-fetch the manifest, compare
  digests; for every addon version newer than the newest ingested one, download + verify + ingest
  as above. Errors (unreachable, parse, bad signature, digest mismatch) mark the source
  `check_failed_at` + reason and change nothing else — last known state keeps serving.
  The poll path is shared: a **manual check** (the buttons below) runs exactly this path with
  `trigger = manual` (vs `scheduled`) in the `addon_source.check` audit row, lease-protected so a
  manual click during a scheduled run does not double-fetch.
- An addon version **removed upstream** never affects installed packages; the source view lists it
  as "removed upstream".
- An addon requiring a newer core (`min_core`) is listed but not ingestable until the instance
  upgrades (state shown with reason).
- **Updates never auto-activate.** The dashboard highlights pending updates (badge on Admin →
  Plugins, list per source); clicking Update runs the ordinary version-activation rules: identical
  or narrower permissions activates on click; widened permissions or a major upgrade goes through
  the existing review (RFE-022 impact, P4-GRANTS conversion warning, approval).
- **Key rotation**: a manifest whose `signing_key` differs from the registered fingerprint never
  auto-trusts: new versions are refused, the source is flagged "signing key changed", and an admin
  must re-confirm (shows old + new fingerprint, updates the `trusted_keys` row, audited).

### Security posture (SEC-86, next free)

- Manifest and package fetches: **https only**, host must not be an IP literal or
  loopback/link-local, timeouts, manifest ≤ 1 MiB, package ≤ `PACKAGE_MAX` (20 MiB), digest
  checked before parse, fetches count against the API's existing outbound guard (`08 §Destination
  guard`). Redirects: the manifest URL is fetched without redirects (it is expected to be a raw
  file), but **package URLs follow at most one redirect** (302 → the release/CDN host, e.g.
  `github.com/…/releases/download/…` → `objects.githubusercontent.com`), still https, still
  same-host-class (public), never to an IP literal or loopback — GitHub release assets and similar
  CDNs redirect by design. The redirect target is not re-fetched through the outbound guard's
  destination policy beyond that check; the digest check remains the integrity anchor.
- A package that fails signature, digest or linter is stored as `invalid` with the reason; nothing
  partial survives.
- The signature always verifies against the **manifest-published key** (via the `trusted_keys` row)
  — a repo cannot silently ship packages signed by a different key.
- Audited (security-critical set): `addon_source.add`, `addon_source.update` (interval, key
  re-confirm), `addon_source.remove`, `addon_source.check`, `addon.ingest`.

### Admin UI (Admin → Addon sources)

List: name, URL, publisher, key fingerprint, schedule, last checked (+ status), addon count. Rows:
Add (flow above), Check now, Edit schedule, Remove (refused while packages from the source are
active; otherwise packages stay but update checking stops, stated in the confirm). Admin → Plugins
gains a **Source** column and **Update available** badges with a "what changed" pointer to the
source's manifest entry.

**Manual check buttons.**
- **Admin → Plugins → "Check for updates"** — forces a check of every addon source at once
  (sequential or fan-out with the shared per-source lease; per-source outcome surfaces in the
  result: updated / up to date / failed + reason). Visible only when at least one addon source is
  registered. `POST /v1/admin/addon-sources/check-all`, audited per source.
- **Admin → Addon sources → row "Check now"** — the same shared check path for one source.

Both buttons disable while their check runs and then refresh the badges/list; the check is
synchronous (sources are few and the fetch is small) — a hanging source fails on its timeout and
reports `check_failed`, never blocks the others.

### Public format documentation

New **`docs/22-addon-repos.md`**: the repo manifest contract (fields, types, limits, versioning and
compatibility rules), how signing works (ed25519, the existing `aurora-sig-v1` package signature —
reference `09 §2`, do not restate), how `aurora plugin build` / `aurora plugin sign` produce the
archives, **where archives are published and how the CI does it** (release assets in the addon
repo's own GitHub releases — the recommended shape; a worked `release.yml` sketch covering
build + verify on push, sign + publish on `v*` tags, secret handling, the no-overwrite rule, and
why Actions run artifacts cannot serve as the manifest's `package` URL), which `plugin.yaml`
fields matter for addons, update/polling semantics from the
publisher's perspective, and a minimal worked example. Linked from `09 §10` (sources table) and the
admin UI's Add dialog ("format documentation").

## Interfaces

- Contracts **C-590..C-609**: admin source CRUD, **check-now (one source) and check-all**,
  source detail (manifest summary, key fingerprint, addons with installed/available versions),
  plugins list provenance + update badges.
- Decision **D-250** (renumbered by the coordinator 2026-11-14: D-159 is taken) records: trust model (key in manifest, TOFU at registration, re-confirm on
  change), never-auto-activate for addon updates, the interval enum + default `1d`, limits.
- `docs/03 §Planned changes` (`addon_sources` table), `docs/05` routes, `docs/09 §10` source row,
  `docs/14 §Admin` (Addon sources page), `docs/15 §Configuration` (the interval key), `docs/17`
  SEC-86, `docs/tests/security-coverage.md`.

## Verification

- Rust: ingestion (happy, bad signature, digest mismatch, oversized, key change), poll state
  machine (unchanged, new versions, removed upstream, unreachable, `never`), trusted_keys
  composition, permission-matrix rows, audit coverage.
- Web: source list/add/check/check-all/remove, update badges, key-change re-confirm; vitest +
  Playwright stage-4 suites.
- E2E: a fixture addon repo (local HTTPS server) end to end: register → approve → use an app →
  publish a new version → poll → update badge → update.
- Guards, OpenAPI + `schema.d.ts` regeneration.

## Dependencies / coordination

- Depends on: nothing in flight; the fixture repo e2e shares the format doc with P5-CERN-ADDONS
  (P5-CERN-ADDONS depends on this task's format doc + ingestion).
- (owner, 2026-11-13) Ordered **after P5-GUIDE-1** (`docs/24-apps-for-agents.md`, app guides on
  demand). Consequences for this task: `docs/22-addon-repos.md` lists the agent guide among the
  `plugin.yaml` fields that matter for addons (`agent.summary`, `agent.instructions` as the
  essentials, `agent.guide[]` with the files of `agent/<app_id>/`), points to `24 §3` for the
  rules without restating them, and shows `aurora plugin guide` in the worked example; the
  fixture addon ships a guide file; the "what changed" view of an update links to the Agent
  guide section of the plugin page (P5-GUIDE-3). The paused lane `alexiri/p5-asrc` rebases on
  P5-GUIDE-1: both edit `services/aurora-api/src/domains/plugins/` and
  `plugin_packages`-adjacent code, and the two must not run at the same time.
  *(Superseded, D-298, 2026-11-15: P5-ASRC merged before P5-GUIDE-1. The `docs/22` and fixture-addon
  items above move to P5-GUIDE-1's scope.)*
- Non-goals: per-source schedules, addon uninstall of packages still connected (existing removal
  rules apply), auto-updates, private/authenticated repos, non-git sources, review of addon
  *content* beyond the existing permissions review.

## Completion report (2026-11-15)
Branch `alexiri/p5-platform`: the paused WIP cherry-picked as `334cac7f` (conflicts were docs only; the migration
renumbered to `20261201000180_addon_sources.sql`, D-159/D-164 → **D-250**), then reviewed hunk by hunk and
finished in `a6dadc84`, `224d8b31` and the gate-fix commit. Contracts C-590..C-609, SEC-86.

**What the review found and changed (WIP → now):**
- *Trust*: the WIP verified a package against the whole trust store. Now a package must be signed by **the
  source's own key** (`signer_not_source_key`), carry the repo's publisher (`publisher_mismatch`), the listed
  id/version (`identity_mismatch`) and an id nobody else owns (`package_id_taken`); the publisher `aurora` is
  reserved; the manifest key is stored in its canonical spelling (FIX-107).
- *Failures*: a version failing digest/signature/identity/lint is stored as an `invalid` row (`status_reason` a
  short code, the sentence in `violations`) and not retried; the WIP stored nothing. The WIP's `status_reason` would
  have broken the ≤ 64 CHECK (found by the IT).
- *Updates never auto-activate*: the WIP only held a **first** version; an update with equal or narrower permissions
  activated itself (the IT caught it). Now every addon version is `pending_approval` (`first_install` /
  `addon_update`); the approval click runs the ordinary activation rules.
- *Dashboard reads, not fetches*: `GET`s fetched the live manifest on every call (N manifests per list). The
  accepted manifest is now stored on the row (`addon_sources.manifest`), the view is built from it, "removed
  upstream" is derived from it, and the plugins list's `source_name` / `update_available` come from it (C-600).
- *Registration*: the WIP fetched the manifest twice (TOCTOU between the key review and the ingest) and swallowed
  ingest errors; one fetch now feeds the transaction (trusted key + source + critical `addon_source.add`) and the
  ingestion; a first registration ingests the **newest** version only (C-607), later checks every newer one
  (the WIP's `semver_newer` ordered a release flag before the numbers — fixed).
- *Audit*: `addon.ingest` joins the security-critical set and commits with the package row; a failed check writes a
  critical `addon_source.check` failure row; a failed fetch is a 200 result, not an error (C-596).
- *Poll*: the config key was dead; it now seeds the schedule of a new source, and registration/interval edits
  schedule the first check (`never` = no schedule). GitHub/GitLab repository pages are normalised to the raw
  manifest URL. `fetch.rs`'s unit test (never run) was wrong and now guards the shape rules.
- *UI*: Admin → Addon sources (list, add with the key review, key re-confirm, schedule, details with removed
  upstream, remove), plugins list Source column, Update-available badge and "Check for updates"; typed DTOs →
  OpenAPI → `schema.d.ts`.

**Deviation to confirm (owner/coordinator):** the brief says "one setting for all sources (per-source override: not
in v1)" **and** lists "Edit schedule" per row, and C-594 is a per-source PATCH. I kept the per-source schedule
(it is what the UI row and the contract need), with the config key `plugins.addon_sources.update_interval` as the
**default for new sources**. If the owner wants one instance-wide value, the PATCH and the column go and the key
becomes the value; nothing else depends on it.

**Verification:** `api it` addons 18/18 (+ the plugins ITs), `addon_sources` unit tests, `aurora-config`,
web vitest 725/725, typecheck, lint, `next build`, Playwright mocked `e2e/addon-sources.spec.ts` 22/22 (both themes,
390 px, axe, dialogs, hostile text). Batch gate: see the lane report.
**Left:** the live "fixture repo over a local HTTPS server" e2e script of the brief's Verification section is
covered by the API IT with an in-process source (register → approve → publish → check → badge → update) rather than a
kind/podman script; "use an app" after install is the ordinary connect flow and not repeated here.
P5-CERN-ADDONS depends on `docs/22-addon-repos.md` (now includes the CLI commands, the release workflow with
verify-on-push, sign-on-tag, the no-overwrite rule and why run artifacts cannot serve) and this ingestion.
