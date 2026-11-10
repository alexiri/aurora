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

- Manifest and package fetches: **https only**, no redirects, host must not be an IP literal or
  loopback/link-local, timeouts, manifest ≤ 1 MiB, package ≤ `PACKAGE_MAX` (20 MiB), digest
  checked before parse, fetches count against the API's existing outbound guard (`08 §Destination
  guard`).
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
archives, which `plugin.yaml` fields matter for addons, update/polling semantics from the
publisher's perspective, and a minimal worked example. Linked from `09 §10` (sources table) and the
admin UI's Add dialog ("format documentation").

## Interfaces

- Contracts **C-590..C-609**: admin source CRUD, **check-now (one source) and check-all**,
  source detail (manifest summary, key fingerprint, addons with installed/available versions),
  plugins list provenance + update badges.
- Decision **D-159** records: trust model (key in manifest, TOFU at registration, re-confirm on
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
- Non-goals: per-source schedules, addon uninstall of packages still connected (existing removal
  rules apply), auto-updates, private/authenticated repos, non-git sources, review of addon
  *content* beyond the existing permissions review.
