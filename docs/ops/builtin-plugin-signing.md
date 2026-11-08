# Built-in app packages: signing and release (P2-CAT-0)

Built-in packages (`plugins/builtin/<id>`, docs/09 §10) ship in the API image at
`/usr/share/aurora/plugins/<id>-<version>.aurora-plugin`. The API loads that directory
(`plugins.builtin_dir`, C-210) through the `builtin` source and **only** accepts archives that
verify against the compiled-in project key (`AURORA_PROJECT_PUBLIC_KEY`, publisher `aurora`).
Unsigned archives, archives signed by any other key (a trusted key included) and invalid archives
are refused and logged (`plugin file refused`); they are never loaded as built-in.

## The project key

| Part | Where |
|---|---|
| Public key | compiled in: `services/aurora-api/src/domains/plugins/keys.rs` (`AURORA_PROJECT_PUBLIC_KEY`, set 2026-11-08) |
| Private key | GitHub Actions secret `AURORA_PROJECT_SIGNING_KEY`, environment `release`; the base64 seed written by `aurora plugin keygen` |

Nobody else needs the private key, and it is never committed. Rotating it means a new public key in
`keys.rs`, a new secret, and a release (older images keep their old archives and key).

## How the pipeline signs

1. `images-api` (push to `master` or a `v*` tag; `environment: release`) writes the secret to a
   0600 temp file in `$RUNNER_TEMP` (an empty file when the secret is absent).
2. The file is passed to the image build as the secret `aurora_project_signing_key`
   (`secret-files`). Nothing is passed as an argument or logged.
3. In the Containerfile stage `plugin-archives`, `scripts/plugins/build-builtin.sh` builds each
   package, runs `aurora plugin sign <archive> --key <file>` and then verifies the archive against
   the public key in `keys.rs`. A secret that is not the private half of that key fails the build.
4. The job deletes the temp file (`if: always()`).
5. **No secret** (for example a fork, or the secret was not created): signing is skipped with a
   warning (`signing SKIPPED ...`), the archives are unsigned, and the API refuses them. Production
   stays fail-closed. Pull requests run `images-api-pr` (no environment) and always take this path.

## First CI run: what to check

- Job `images-api` on the first `master` push after merge: the `project signing key` step logs
  `signing: AURORA_PROJECT_SIGNING_KEY present, built-in plugins will be signed` (not the
  `::warning:: signing SKIPPED` annotation).
- In the `build and push` log (stage `plugin-archives`): `signing: project key present`,
  `signed   /out-plugins/gitlab-1.0.0.aurora-plugin (verified against the compiled-in project public key)`.
- After the image is published, `podman run --rm --entrypoint ... ` is not possible on distroless;
  instead start the API from that image against a database and look for the `gitlab` package in
  `GET /v1/admin/plugins` with `source: builtin`, `status: active`, `signer.kind: builtin`, and no
  `plugin file refused` warnings in the API log.
- Job `rust-build`: step `Built-in plugin checks` is green; `images-api-pr` and the other image jobs
  still build on pull requests.

## What the owner provides

- The secret `AURORA_PROJECT_SIGNING_KEY` in the GitHub environment `release` (done 2026-11-08).
- Restrict the `release` environment to `master` and `v*` tags if desired; `images-api` runs only on
  push events, and pull requests never use the environment.
- Nothing else for built-ins. Trusted publisher keys for third-party packages are a separate
  admin function (Admin, Trusted keys) and cannot sign built-ins.

## Dev and e2e

The builtin directory is not used for development: it rejects unsigned archives by design. To try the
packages locally, build unsigned archives and load them through the ordinary plugin directory:

```sh
cargo build -p aurora-cli
bash scripts/plugins/build-builtin.sh --out /var/lib/aurora/plugins   # unsigned, says so
# plugins.allow_unsigned = true (dev only); the package lands as `pending_approval` (first_install)
# from source `directory`, because only project-signed builtin archives auto-activate.
```

To test signing without the real key: `aurora plugin keygen --out <file outside the repo>`, then
`PUBKEY_OVERRIDE="$(cat <file>.pub)" bash scripts/plugins/build-builtin.sh --out <dir> --key-file <file>`.

## Local checks

`bash scripts/plugins/check-builtin.sh` (validate, test, fixture coverage, scopes, token, docs; CI
runs it in `rust-build`), `bash scripts/guards/run-all.sh` (includes the live-token guard) and
`bash scripts/plugins/check-links.sh` (network; weekly `plugin-links` workflow, non-blocking).
