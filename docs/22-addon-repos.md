# 22. Addon repositories (P5-ASRC, D-250)

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

One YAML file per repository, ≤ 1 MiB, served over **https** as a raw file at a
stable URL (the repository root is the convention). An admin may paste the
repository page — `https://github.com/<owner>/<repo>` (also `/tree/<ref>/<dir>`) or a
GitLab project URL — or the direct link to `aurora-addons.yaml`; Aurora stores the raw
manifest URL (`raw.githubusercontent.com/<owner>/<repo>/HEAD/aurora-addons.yaml`,
`<gitlab>/<project>/-/raw/HEAD/aurora-addons.yaml`). Strict parse:
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

A version is an ordinary signed package archive (`09 §2`), built with the `aurora`
CLI (`docs/operator/plugins-and-connectors.md` lists the commands):

```bash
aurora plugin keygen --out cern-release.key          # once; writes cern-release.key and cern-release.key.pub
aurora plugin build indigo_sso --out indigo_sso-1.0.0.aurora-plugin
aurora plugin validate indigo_sso-1.0.0.aurora-plugin --allow-unsigned   # the linter, before signing
aurora plugin sign indigo_sso-1.0.0.aurora-plugin --key cern-release.key
sha256sum indigo_sso-1.0.0.aurora-plugin             # → the manifest's sha256
```

`cern-release.key.pub` is the manifest's `signing_key` (base64). What a package must
satisfy to be ingested from a repository — beyond everything `09` already requires:

| Rule | Refused with |
|---|---|
| The signature verifies against **the manifest's `signing_key`**. A package signed by any other key — the Aurora project key, another key the admin trusts for the same publisher, a key another repo published — is refused. | `signer_not_source_key` |
| `plugin.yaml` `package.publisher` is the manifest's `publisher`, and `package.id` / `package.version` are the id and version the manifest lists. | `publisher_mismatch`, `identity_mismatch` |
| The package id does not belong to another repository, another publisher or a package that was uploaded or ships with Aurora. | `package_id_taken` |
| The archive's sha256 is the manifest's `sha256` (checked before anything is parsed). | `digest_mismatch` |

A refused version is stored as an `invalid` package with its reason (the admin sees it
on the source and in the plugins list) and is **not retried** by later checks: publish a
new version to fix it. The other versions of the check go on. The publisher `aurora` is
reserved for the project.

## Publishing a release

The recommended shape is **the addon repository's own GitHub releases**: the manifest
lives in the repository root, every version is a release asset. A workflow sketch
(`.github/workflows/release.yml`) with the three jobs that matter:

```yaml
name: release
on:
  push:
    branches: [main]          # build + verify on every push, publish nothing
    tags: ["v*"]              # sign + publish on a version tag
permissions:
  contents: read
jobs:
  build:                      # no secret in this job
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: cargo install --locked aurora-cli     # or download the release binary of the Aurora you target
      - run: |
          mkdir dist
          for d in */plugin.yaml; do
            aurora plugin build "$(dirname "$d")" --out "dist/$(dirname "$d").aurora-plugin"
          done
      - run: for p in dist/*.aurora-plugin; do aurora plugin validate "$p" --allow-unsigned; done
      - run: |
          # the manifest must list exactly what was built: same ids, versions and sha256
          ./scripts/check-manifest.sh dist aurora-addons.yaml
      - uses: actions/upload-artifact@v4          # hand-over to the next job only
        with: { name: dist, path: dist }
  release:
    needs: build
    if: startsWith(github.ref, 'refs/tags/v')
    runs-on: ubuntu-latest
    environment: release                           # the signing key is a secret of this environment only
    permissions:
      contents: write
    steps:
      - uses: actions/download-artifact@v4
        with: { name: dist, path: dist }
      - run: |
          key="$(mktemp)"; trap 'rm -f "$key"' EXIT; chmod 600 "$key"
          printf '%s' "$RELEASE_SIGNING_KEY" > "$key"
          for p in dist/*.aurora-plugin; do aurora plugin sign "$p" --key "$key"; done
        env:
          RELEASE_SIGNING_KEY: ${{ secrets.RELEASE_SIGNING_KEY }}
      - run: |
          # no-overwrite rule: a published version is never replaced
          for p in dist/*.aurora-plugin; do
            gh release view "${GITHUB_REF_NAME}" --json assets -q '.assets[].name' | grep -qx "$(basename "$p")" \
              && { echo "$(basename "$p") is already published"; exit 1; }
          done
          gh release create "${GITHUB_REF_NAME}" dist/*.aurora-plugin --title "${GITHUB_REF_NAME}" --notes "" \
            || gh release upload "${GITHUB_REF_NAME}" dist/*.aurora-plugin   # never --clobber
        env:
          GH_TOKEN: ${{ github.token }}
      - run: sha256sum dist/*.aurora-plugin        # the values for aurora-addons.yaml
```

Then add the version to `aurora-addons.yaml` on your default branch (the `package` URL points
at the release asset, `https://github.com/<owner>/<repo>/releases/download/<tag>/<file>`; GitHub
redirects it once to its CDN, which Aurora follows) and commit. Points that matter:

- **Secrets**: only the `release` job, only for a tag, sees the signing key
  (`environment: release` with required reviewers, if you want a human gate); the build job
  and every pull request never do, so code under review cannot read it. The key is written to
  a `0600` temp file and removed. Keep an offline copy; a lost key means a key rotation (below).
- **The no-overwrite rule**: a version, once published, is immutable — Aurora keys a package by
  `(id, version, digest)` and installed copies would diverge from a replaced asset. Fix a
  mistake with a new version number; never `--clobber`, never move a tag.
- **Why the manifest points at release URLs instead of Actions run artifacts**: a workflow
  run's artifact needs an authenticated token to download, its URL is not stable, it is a
  zip of files rather than the archive, and it expires (90 days by default) — while an Aurora
  instance must be able to fetch every package with no credentials at all (the TOFU model's
  only secret is the publisher's signing key). Release assets (or any static file server,
  object storage, your own CDN) are public, permanent and cacheable.

## What a check does (the publisher's view)

A poll (on the admin's schedule: `6h`, `12h`, `1d`, `1w`, `2w`, `1mo`, `never`) or the
admin's "Check now" re-reads `aurora-addons.yaml` — fetched **without redirects**, ≤ 1 MiB —
and compares it with what the instance holds:

- **First registration** ingests the **newest** version of each addon that this Aurora can run
  (`min_core`), not the whole history. **Later checks** ingest every version newer than the
  newest one the instance holds, oldest first.
- Each version is downloaded (https, a public host, at most **one** redirect — the GitHub
  release-asset pattern — and ≤ 20 MiB), its **sha256 is checked against the manifest before
  anything is parsed**, then it goes through the ordinary ingest (signature, linter, approval).
- **Nothing activates by itself.** A new addon's first version, and every update, waits as
  `pending_approval`: the admin sees "Update available" on the plugins list and the source,
  and approving the new version runs the ordinary activation rules (identical or narrower
  permissions activate on click; widened permissions or a major version go through the review).
- A version you **delete from the manifest** stays installed everywhere (the admin sees it as
  "removed upstream"); an existing `(package, version, digest)` is never replaced.
- A repository that is unreachable, or whose manifest is invalid, changes nothing: the check
  records the (sanitized) reason and the last known state keeps serving.
- A manifest whose `signing_key` or `publisher` differs from the registered one is never
  trusted automatically (see "Rotating the signing key").

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
