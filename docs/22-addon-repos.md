# 22. Addon repositories (P5-ASRC, D-164)

How to publish Aurora app packages as a third-party repository that an Aurora
instance administrator can register as an **addon source**.

Owner doc for the source row: `09 §10` (source `addon`), `03 §addon_sources`,
`05 §Instance administration` (the admin surface), `02 §Limits` (`ADDON_*`).
Security posture: `17` SEC-86, `06 §Destination guard`.

## Principles

1. **The manifest is the trust anchor.** The `signing_key` published in the
   manifest is the only key whose packages the repository can ship. An admin
   registering the repo reviews the key's fingerprint (TOFU); a repo that later
   publishes a different key is flagged and waits for an explicit re-confirm —
   it can never silently mint packages under the new key.
2. **Packages are ordinary packages.** Everything in `09` applies unchanged:
   manifest contract, linter, signatures (`09 §2`), approval, activation,
   revocation. Addon packages are distinguishable only by
   `source = "addon"` + `source_id` (provenance in the dashboard).
3. **No auto-activation for third-party code.** The first version of every
   addon package is `pending_approval`; updates go through the ordinary rules
   (identical/narrower permissions activate automatically, widened or major
   versions need approval).
4. **Removal is additive.** A version deleted from the manifest never touches
   installed packages; it only stops being offered to new installs.

## The manifest: `aurora-addons.yaml`

One YAML file per repository, ≤ 1 MiB, served over **https** (a raw file —
put it in the repo root or a `docs/` folder and link the raw URL, e.g. the
GitHub `raw.githubusercontent.com` URL of your default branch). Strict parse:
unknown fields are refused, so always extend with a new `api` version, never
silently.

```yaml
# The manifest contract this file follows. Integer. Aurora refuses values it
# does not understand (a newer api means "upgrade Aurora first").
api: 1

# The repository, shown to the reviewing admin.
repo:
  name: CERN addons            # 1..80 characters, required
  description: >-              # ≤ 280 characters
    App packages maintained by the CERN Aurora team.
  homepage: https://aurora.cern/docs/addons   # https URL, optional
  contact: mailto:aurora-admin@cern.example   # https or mailto URL, optional

# The package publisher every addon in this repository uses. One repo = one
# publisher (the key belongs to the publisher in `trusted_keys`).
publisher: cern

# Base64 ed25519 **public** key (the trust anchor, see `09 §2 SIGNATURE`).
# Everything the repository ships must be signed with the matching private
# key. The admin sees sha256:<first 12 hex of the key's digest> at
# registration and re-confirmation — changing this value freezes the source.
signing_key: "H+zP/3e/AvDIupz+XgxH31uErU7l4ySFFV9lv9qiOpk="
signing_key_description: "CERN release key (rotated 2027-01)"  # ≤ 80, optional

# The addons. At most 100 entries; at most 50 versions per addon.
addons:
  - id: indigo_sso             # a valid package id ([a-z][a-z0-9_-]*)
    name: Indigo SSO
    description: Single sign-on for Indigo services.   # ≤ 280 characters
    versions:
      "1.0.0":                 # semver (keys are strings — quote them!)
        package: https://github.com/cern/aurora-addons/releases/download/v1.0.0/indigo_sso-1.0.0.aurora-plugin
        sha256: 9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08
        size_bytes: 184320     # advisory; the download is capped at PACKAGE_MAX anyway
        min_core: "0.4.0"      # optional: the oldest core this version runs on
```

Rules the parser enforces (`422 addon_manifest_invalid` otherwise): `api`
must be exactly the version this Aurora understands; `repo.name`, addon
`id`/`name`, `publisher` and key formats as annotated; every version key must
be valid semver; every `package` an https URL; every `sha256` 64 hex
characters; `size_bytes` ≤ `PACKAGE_MAX` (20 MiB).

## Building and signing a version

A version is an ordinary signed package archive (`09 §2`):

```bash
aurora-pkg build indigo_sso/plugin.yaml -o indigo_sso-1.0.0.aurora-plugin
aurora-pkg sign indigo_sso-1.0.0.aurora-plugin --key cern-release.key
sha256sum indigo_sso-1.0.0.aurora-plugin   # → the manifest's sha256
```

The signature must verify against the manifest's `signing_key`. A package
signed by any other key — including the Aurora project key or a key another
repo published — is refused.

## Publishing a release

Recommended shape (GitHub Actions):

```yaml
# .github/workflows/release.yml (sketch)
name: release
on:
  push:
    tags: ["v*"]
jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: aurora-pkg build indigo_sso/plugin.yaml -o indigo_sso-${GITHUB_REF_NAME#v}.aurora-plugin
      - run: aurora-pkg sign indigo_sso-*.aurora-plugin --key <($CERN_RELEASE_KEY)
        env:
          CERN_RELEASE_KEY: ${{ secrets.CERN_RELEASE_KEY }}
      - run: sha256sum indigo_sso-*.aurora-plugin
      - uses: softprops/action-gh-release@v2
        with:
          files: indigo_sso-*.aurora-plugin
```

Then add the version to `aurora-addons.yaml` on your default branch (the
`package` URL points at the release asset) and commit. **Why the manifest
points at release URLs instead of Actions artifacts**: workflow artifacts need
an authenticated token to download and their URLs are not stable — an Aurora
instance must be able to fetch every package with no credentials at all
(the whole point of the TOFU model is that the *only* secret in the chain is
the publisher's signing key). Release assets (or any static file server,
object storage, your own CDN) are public, permanent and cacheable.

A poll (or the admin's "Check now") then ingests the new version: digest
checked first, signature against your published key, ordinary approval flow
afterwards. Nothing is overwritten: an existing `(package, version)` row wins,
and versions you delete from the manifest stay installed everywhere.

## What an admin sees

- **Add**: paste the repo URL (`https://github.com/cern/aurora-addons`) or the
  direct manifest URL; Aurora normalizes to `…/aurora-addons.yaml` and shows
  the review screen — repo info, publisher, the **signing-key fingerprint**
  prominently ("trust this key?"), the addon list with versions, and how many
  packages are new to the instance.
- **Run**: list with status chip (`ok` / `check_failed` + sanitized reason /
  `signing_key_changed` banner), "Check now", schedule (`6h`, `12h`, `1d`,
  `1w`, `2w`, `1mo`, `never`), remove (refused while packages of the source
  are active; packages otherwise stay), re-confirm key (old vs new fingerprint
  side by side).
- **Plugins**: addon rows carry the source's name, "Update available" badges
  and a "Check for updates" button.

## Rotating the signing key

1. Publish a manifest whose `signing_key` names the new key **and sign the new
   packages with it**.
2. The next check flags the source `signing_key_changed`; nothing new is
   ingested; installed packages keep working.
3. An admin compares old and new fingerprints, verifies the new one out of
   band (your `contact`), and confirms. Aurora updates the source and its
   `trusted_keys` row in one audited transaction; the next poll ingests the
   queued versions.

Never reuse a key across publishers, and never publish a key whose private
half lives anywhere less protected than CI secrets (`08 §Credentials`).
