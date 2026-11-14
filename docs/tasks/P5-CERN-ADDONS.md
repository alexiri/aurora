# P5-CERN-ADDONS: `aurora-addons-cern` — CERN Phonebook and CERN Maps (owner, 2026-11-10)

The first addon repo for P5-ASRC, in the existing `aurora-addons-cern` repository
(`github.com:alexiri/aurora-addons-cern`, branch `main`, currently empty), plus two real addons.

Depends on P5-ASRC (format documentation, ingestion, admin UI). The two tasks can start together
only where the addon content is concerned; the repo manifest's first real-world validation is this
task.

## Part A — the repo

Create in `aurora-addons-cern`:

- **`aurora-addons.yaml`** — the P5-ASRC repo manifest: repo info, `publisher: cern` (id
  `[a-z][a-z0-9_-]{1,47}`), the repo's **ed25519 signing key** (a dedicated key for this repo;
  the private half lives in a GitHub Actions secret in the `release` environment and the owner's
  local copy — never committed), and the two addon entries below.
- `README.md` — what this repo is, how to register it in Aurora, contact.
- `plugins/cern-phonebook/` and `plugins/cern-maps/` — the two packages (layout per
  `plugins/builtin/README.md`: `plugin.yaml`, `assets/icon.png` (+ dark), `docs/setup.md`,
  `tests/*.yaml`).

Tooling: build and sign with the existing CLI (`aurora plugin build`, `aurora plugin sign`,
key via `aurora plugin keygen`/`sign`), fixture-run with `aurora plugin test`. A small
`scripts/release.sh` rebuilds both packages and regenerates the manifest (versions, sha256, size).

### CI and publishing

GitHub Actions, two jobs in one `release.yml` workflow, sharing `scripts/release.sh`:

- **On every push (and pull request) to `main` — build + verify, no publishing:** build both
  packages, `aurora plugin test` fixture runs, verify each signature against the manifest's public
  key, regen the manifest and fail on drift (diff only; **no auto-commit** — the manifest is
  regenerated and committed by the owner locally as part of `scripts/release.sh`, keeping every
  manifest change reviewed like any other edit). Builds on pull requests run unsigned.
- **On a `v*` tag — publish:** the same build + verify, then sign with the repo private key from
  the `AURORA_CERN_SIGNING_KEY` Actions secret (environment `release`, restricted to tags) and
  create a GitHub **Release** for the tag holding the two signed `.aurora-plugin` archives as
  release assets. The release body lists addon ids, versions, sha256 digests and sizes (generated
  from the manifest).
- The manifest's version URLs point at the **release-asset download URLs**
  (`https://github.com/alexiri/aurora-addons-cern/releases/download/v<version>/<id>-<version>.aurora-plugin`,
  one release per addon version tag) — stable, public, anonymous-https URLs that Aurora's addon
  ingestion can fetch. **Actions run artifacts are not a substitute**: their download URLs need an
  authenticated API call, so they cannot serve as the manifest's `package` URL. Run artifacts may
  still be uploaded on non-tag builds for convenience (never referenced by the manifest).
- Digest discipline: the sha256 in a manifest version entry is computed from the exact archive the
  release asset carries (built in the same job that publishes it), so a poll of the source can
  never see a manifest digest that disagrees with the downloadable archive. Re-running the publish
  for an existing tag **fails** instead of overwriting (release and assets already exist) — a
  version is immutable once published; fixes go out as a new version.
- Secret handling mirrors `docs/ops/builtin-plugin-signing.md`: the key reaches the signer as a
  0600 file in `$RUNNER_TEMP`, passed as a secret (never an argument), deleted `if: always()`;
  missing secret (fork, PR) = signing skipped with a warning, build still green, nothing published.
  The signing key is this repo's own — the Aurora project key never signs addons.

## Part B — CERN Phonebook (`cern-phonebook`)

People lookup over **CERN's central LDAP service** (`xldap.cern.ch`, LDAPS :636, anonymous read —
the service officially offers anonymous read access to FIM/AD user and e-group data; the CERN
SSO-walled phonebook web UI is out of scope).

Use cases (owner): find a person's **e-mail address**; "where does so-and-so work" / "where is
their office"; **who is someone's boss** (manager chain). Explicitly out of scope (owner):
contract status, nationality.

### Transport: the LDAP tool lane

LDAP is not HTTP, so a governed HTTP tool cannot reach it. Add a **virtual-host LDAP lane** to the
gateway — the same pattern as the phase-2 database lane (`21 §Database lane`): package tools
classify an app's requests on a synthetic host (`ldap.aurora.internal`), the gateway translates the
admitted request into a real LDAP operation (anonymous bind + search, :636, TLS certificate
verified, **read-only**: search/compare only, no write operations can be expressed), injects it on
the app's connection, maps results back to JSON. No policy, approval, rate-limit or request-log
changes — all reuse the HTTP machinery unchanged.

Simpler fallback if the lane proves too heavy for phase 5: a single declarative `ldap_search` tool
executed **in the sandbox** (agent-side binary, no secrets needed for anonymous binds), with the
host allow-listed in the manifest so the gateway still sees and logs egress. Trade-off documented
in the brief for the owner to pick at implementation start. **Default: the gateway lane.**
**Owner, 2026-11-14 (D-235(d)): the gateway lane.**

### Tools (permission catalogue)

All read tools; the connector host is `xldap.cern.ch` via the LDAP lane:

- `search_people(query, limit)` — filter `(|(displayName=~q*)(mail=~q*)(cn=~q*))` (indexed attrs);
  returns name, mail, username, department, office, phone.
- `get_person(username)` — exact `uid` lookup, full useful attribute set.
- `get_manager_chain(username, max_hops ≤ 5)` — follow `manager`/`cernReportTo` references.
- `search_groups(query, limit)` / `get_group_members(group)` — e-group lookup (useful for
  "who is in team X" and for reaching people via a group).

Attributes exposed map to what pyphonebook-style queries use (displayName, mail, telephoneNumber,
physicalDeliveryOfficeName, cernDepartment, cernGroup, cernSection, manager). `get_person` and the
chain tools resolve `manager` **by reference** (one extra lookup per hop, bounded, cached per turn)
so answers contain real names, not DNs. All results include a `phonebook.cern.ch` deep link.

### Tests

`tests/*.yaml` fixtures with a recorded-LDAP harness (same philosophy as the HTTP cassettes of
`09 §12`): a bundled OpenLDAP dev instance seeded with a tiny fixture DIT, or recorded LDAPS
transcripts — implementation choice recorded in the lane report. Negative fixtures: oversized
filters, refused anonymous-write, host mismatch.

## Part C — CERN Maps (`cern-maps`)

All read tools against the **keyless public APIs** (verified live, 2026-11-10):

- **Search**: `GET https://maps.cern.ch/api/search?query=…&filterBy=&resultOffset=0&resultRecordCount=N`
  (Typesense-style; returns `found`, `hits[]` with `label`, `category_type_en/fr`, `location`
  **[lat, lon]**, `search_id`, per-hit ArcGIS `url`).
- **Feature details**: the per-hit ArcGIS FeatureServer
  (`maps.cern.ch/arcgis/rest/services/Hosted/LOC_MAPCERN/FeatureServer/0/query?where=OBJECTID='…'&f=json`)
  — building polygons, room/POI attributes (`numero`, `etage`, `type`, `sigle`), Swiss coordinates
  (WKID 2056).
- **Geocoding**: `GeocodeServer` (`maps.cern.ch/arcgis/rest/services/Batiments/GeocodeServer`).
- **Routing**: `NAServer` (`maps.cern.ch/arcgis/rest/services/Routing/NetworkAnalysis/NAServer/Route`)
  — real directions between two map features, travel times per mode.
- **Map links** (no API): build `maps.cern.ch/?n=…&travelMode=…&itinerary=…` links per
  `maps.docs.cern.ch/URLParameters.html` for the chat UI ("open in CERN Maps").

### Tools

- `search_places(query, category?, limit?)` — the search API; each result carries lat/lon, type,
  building, and its `search_id`.
- `get_place(search_id)` — feature details (attributes + geometry summary).
- `geocode(query)` — building geocoder.
- `get_directions(from, to, mode)` — NAServer route: steps, distance, **travel time** per mode
  (walk/bike/car); `from`/`to` accept a `search_id`, lat/lon or building number (resolved via
  search/geocode first).
- `build_map_link(target, options?)` — deep link per URLParameters (center, search, popup content,
  transport, 3D…).

Hosts: `maps.cern.ch` (search, arcgis REST). The ArcGIS API surface is broad — the manifest's
`hosts[]` + tools restrict requests to the query patterns the tool catalogue declares (path/param
classification like every HTTP app), so uncatalogued ArcGIS calls fail closed.

### Tests

Fixture cassettes for search (buildings, rooms, POIs), feature query, geocode, one route; negative
fixtures for unknown `search_id` and oversized responses.

## Part D — owners' notes

- The repo's signing key is the repo's own (not the Aurora project key); registration in P5-ASRC
  records its fingerprint at add time (TOFU).
- Both packages are **credential-free** (anonymous LDAP, keyless HTTP): connect flow is
  `fields` with zero or trivial fields, so an org admin can connect without CERN SSO. If CERN later
  requires authenticated phonebook access, that is a new decision (SSO connection via existing OIDC
  machinery) — not in this task.
- Icons: CERN's official marks are trademarked — use simple original monogram/line icons (CERN's
  own icon usage rules are restrictive; flag for owner review before shipping).

## Verification

- Package level: `aurora plugin test` fixture suites for both packages; `check-builtin.sh`-style
  local checks (icons, setup guides, no stray files).
- CI: a dry run of the workflow (build + verify, no tag) locally (`act` or step-by-step); on the
  first real run, check the release exists with both assets, the asset URLs in the manifest
  resolve, an Aurora instance registers the source and ingests both packages, and a re-tag attempt
  fails instead of overwriting.
- E2E (kind): register `aurora-addons-cern` as an addon source → approve both packages → connect →
  agent answers "What is X's e-mail and where do they sit?" and "How do I get from building 513 to
  the main auditorium, and how long does it take?" through the gateway.
- LDAP lane (if chosen): gateway tests mirroring the database lane's admission matrix.

## Dependencies / coordination

- Depends on: P5-ASRC (format doc + ingestion + admin UI).
- Non-goals: contract status / nationality attributes (owner), write operations of any kind,
  Mattermost/channel presence, indoor turn-by-turn navigation, cached local copies of directory
  data.
