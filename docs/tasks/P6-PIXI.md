# P6-PIXI: Agents add tools from conda-forge with pixi, one-off by default (2026-11-28; owner)

## Why

Agents keep hitting missing tools. One wanted `yq`; another wanted ImageMagick to convert an image. The image
can't hold everything, and the agent can't use dnf: it has no root, the root filesystem is read-only, and
anything installed there would be lost at the next boot (D-516). Python, Node.js and (with FIX-298) Ruby
packages already install into the home. Command-line tools written in other languages have no such route.

conda-forge has about 25,000 packages, each built so it can be installed into any folder. pixi installs them
as an ordinary user, into the home, with no root (D-517). Examples on conda-forge (checked 2026-11-28):
- `go-yq` (Mike Farah's yq; conda-forge's `yq` is a different tool, a Python wrapper around jq)
- `imagemagick`, `libvips`, `ffmpeg`, `librsvg`, `ghostscript`, `exiftool`, `tesseract`
- `pandoc`, `typst`, `tectonic`, `poppler`, `qpdf`, `graphviz`
- `terraform`, `opentofu`, `kubernetes-helm`, `kubernetes-client`, `gh`, `trivy`
- `go`, `rust`, `openjdk`
- `duckdb`, `miller`, `csvkit`, `shellcheck`, `vale`

**The owner's rule:** agents run a tool once, without installing it (`pixi exec`). They install it
(`pixi global install`) only when they will likely need it again in later sessions.

## Change

### 1. Image (`images/sandbox/Containerfile`, `base` stage)
- **The pixi binary.** Install it at `/usr/local/bin/pixi`.
  - Use the static musl build for `x86_64` and `aarch64`, picked by `uname -m` with a SHA-256 per
    architecture, the same pattern as the codex, goose and opencode downloads.
  - Pin the newest release published **at least 14 days before the lane starts**.
  - Use build arguments `PIXI_VERSION`, `PIXI_SHA256_X86_64` and `PIXI_SHA256_AARCH64`.
  - The build checks the digest and runs `pixi --version`.
- **System configuration `/etc/pixi/config.toml`** (image-owned; the agent can't write it):
  - `default-channels = ["conda-forge"]`. Never Anaconda's `defaults` channel, which has commercial licence
    terms for large organisations.
  - `exclude-newer = "7d"`. New `pixi exec` solves and new global manifests skip packages published in the
    last 7 days, so a compromised release is usually pulled before an agent can install it. The agent can
    override this with a flag; it is a default, not a control.
  - `run-post-link-scripts = "false"` (pixi's default, stated explicitly so a later pixi can't change it).
  - `[cache] netfs-redirect = "never"`. The home may be a network filesystem. Under `auto`, pixi would move
    caches to `$TMPDIR`, which is the 512 MiB memory-backed `/tmp`.
  - `tls-root-certs` stays at its default: the sandbox's `SSL_CERT_FILE` (the gateway CA) takes precedence,
    as pixi documents.
  - **Mirror:** an optional build argument `SANDBOX_CONDA_MIRROR` (empty by default). When set, it renders
    `[mirrors] "https://conda.anaconda.org/conda-forge" = ["<value>"]`, so a deployment can point at an internal
    mirror (Artifactory, Nexus or prefix.dev's) the same way it overrides `SANDBOX_BASE_IMAGE`.
- **Environment (image `ENV`, and every spawn path FIX-298 checked):**
  - `PIXI_HOME=/workspace/.pixi`: global environments and the global manifest `manifests/pixi-global.toml`.
  - `PIXI_CACHE_DIR=/workspace/.cache/pixi`: the package cache, repodata, and the `pixi exec` environments.
  - `/workspace/.pixi/bin` joins the one `PATH` right after FIX-298's gem `bin`, in all five places
    (Containerfile `ENV`, `aurora.sh`, `zshenv`, `spawn::IMAGE_PATH`, and the "keep identical" comments). The
    result:
    `/workspace/.local/bin:/workspace/.local/share/gem/bin:/workspace/.pixi/bin:/opt/aurora/bin:/usr/local/bin:/usr/bin:/bin`.

### 2. Supervisor: the one-off cache stays bounded (`services/aurora-supervisor`)
One-off runs are cached in `PIXI_CACHE_DIR`. Without a bound, that cache grows on the agent's volume forever.
- **At each boot, before the first agent process starts,** measure the cache's apparent size (`metadata.len()`,
  so sparse files count).
  - Stop counting once the size exceeds `TOOL_CACHE_MAX` (2 GiB, a constant, no setting) or 200,000 entries.
  - If either limit is exceeded, **rename** `.cache/pixi` to `.cache/pixi.old-<boot short id>` and delete
    that folder in a background task. Do the same for any leftover `pixi.old-*`.
  - The rename is instant, so the boot never waits on the delete.
- Global environments link their files to the package cache with hard links, so deleting the cache leaves every
  installed tool working.
- **The home belongs to the agent** (FIX-297).
  - Measure, rename and delete beneath the home's descriptor with the no-follow helpers. Never follow a link,
    and never open a FIFO or device for reading.
  - A `.cache` or `.cache/pixi` that is a link is removed as a link. Its target is never touched.
  - Errors are logged and skipped. A failed prune never fails a boot.
- Global installs (`.pixi`) are never pruned. They are what the agent chose to keep, and it removes them
  itself.

### 3. Instructions to the agent (`services/aurora-supervisor/src/instructions.rs`)
Add a section **only when `/usr/local/bin/pixi` exists** in the sandbox, checked like the `agent_cli` flag, so
an older or custom image says nothing. This is agent-facing text: state facts only, with no task, decision or
document references (D-483). Proposed wording (keep its meaning; tighten if you can):

> ## Adding tools
>
> You have no root and no system package manager, but you can add command-line tools yourself.
> To use a tool for the task at hand, run it without installing it:
> `pixi exec --spec <package> -- <command> …`
> For example: `pixi exec --spec imagemagick -- magick in.png out.webp`.
> It downloads the first time and is cached for a while.
> Install a tool with `pixi global install <package>` only when you will keep needing it in later sessions,
> such as a linter for a repository you work on. Installed tools stay on your `PATH`.
> Remove the ones you no longer need with `pixi global uninstall <package>`: they use your disk.
> Package names come from conda-forge: `pixi search <name>` finds them. The usual `yq` is `go-yq`.
> For Python tools use `uvx` (one-off) or `uv tool install`; for Node.js, `npx` or `npm install -g`;
> for Ruby, `gem install`.

Check every command in this text in the image before you commit it, including the `--` separator and the
`search` output. Golden-test the section, and test that it is absent when pixi isn't in the image.

### 4. Network
Packages come from `conda.anaconda.org` through the loopback proxy (pixi reads `HTTPS_PROXY`).
- Confirm every host a `pixi exec` and a `pixi global install` contact. Expect `conda.anaconda.org` only;
  report anything else (prefix.dev, a CDN, PyPI for mapping data).
- Add the hosts to the `15 §Governance example` `baseline_allowlist`. No default changes.
- If pixi fetches a conda-to-PyPI name mapping on its own, turn that off in the system config (or explain in
  the report why it's needed).
- **Sharded repodata (added 2026-11-28, owner).** conda-forge's index comes in two forms. The full
  `repodata.json.zst` is 61 MB for `linux-64` plus 30 MB for `noarch` (measured 2026-11-28). The sharded form
  is a 0.6 MB `repodata_shards.msgpack.zst` plus only the shards of the packages being solved.
  `conda.anaconda.org` and `prefix.dev` both serve the sharded form. Every agent whose cache was just pruned
  solves again, so the full form would cost about 90 MB per agent per solve.
  - Confirm that pixi fetches the sharded form from `conda.anaconda.org` and never the full
    `repodata.json(.zst)`. pixi's configuration example enables sharding per host only for `prefix.dev`.
  - If it isn't on by default for `conda.anaconda.org`, enable it in `/etc/pixi/config.toml` with
    `[repodata-config."https://conda.anaconda.org"] disable-sharded = false`.
  - When `SANDBOX_CONDA_MIRROR` is set, render the same setting for the mirror's URL. Confirm that pixi falls
    back to the full index when a mirror has no shards, rather than failing.
  - Report the bytes a first `pixi exec --spec go-yq` downloads.

### 5. Docs
- `11 §Sandbox image`: pixi, its configuration and mirror argument, the two variables, the `PATH`, and the
  durable-installs line (`pixi global install`; one-off runs are cached and pruned at boot).
- `12 §Terminal` (`PATH`); `12` instruction-file section: the new section and its condition.
- `15`: the allowlist example and `SANDBOX_CONDA_MIRROR`.
- `17-security.md`: the SEC-174 row (below).

## Scale (D-517)
- **Nothing reaches the database or the blob store.** A home is a resident volume (a PVC, or a named podman
  volume). Stopping keeps it, deleting the agent destroys it, and neither the agent export nor a template
  copies it.
- The costs that grow are:
  1. **Disk:** each agent's volume holds its own tools, with no sharing between agents. A writable shared
     cache would let one agent plant files for another. Global installs are bounded by the volume's size (an
  install fails with "no space"), and the one-off cache by `TOOL_CACHE_MAX`.
  2. **Downloads:** each first use downloads through the gateway, at the gateway's TLS-interception cost. The
     sharded index keeps each solve's index download well under a megabyte instead of about 90 MB
     (§4). At scale, a deployment sets `SANDBOX_CONDA_MIRROR` to a nearby mirror.
- **Constraint for later:** if homes ever move over the network (the deferred `snapshot` durability, D-379),
  that move excludes `.cache/` and may rebuild `.pixi/` from `manifests/pixi-global.toml` with
  `pixi global sync`, instead of copying environments.

## Tests (each must fail first)
- **Hosted e2e, offline,** on the full image as the agent's uid on the read-only root:
  - Build a minimal conda package in the test: a `noarch` package with one shell script in `bin/`, plus
    `info/index.json` and `info/paths.json`, and a `repodata.json` for a `file://` channel under `/tmp`.
  - `pixi exec -c file:///tmp/chan --spec hello -- hello` prints its line.
  - `pixi global install -c file:///tmp/chan hello`, then `hello` runs by bare name from a login bash and from
    a turn's `process_start`.
  - Restart the sandbox: `hello` still runs.
  - Plant a 3 GiB sparse file in `.cache/pixi` and restart. The cache is gone (or renamed and being deleted),
    `hello` still runs, and nothing outside the home changed.
- **Supervisor unit tests** (prune):
  - Under budget, nothing changes.
  - Over budget, the folder is renamed and deleted.
  - Over 200,000 entries counts as over budget.
  - `.cache` as a link to a folder outside the home: the outside folder is untouched.
  - A FIFO in the cache doesn't block.
  - Leftover `pixi.old-*` folders are removed.
  - A failing delete doesn't fail the boot.
- **Instructions:** the golden text, and its absence without the binary.
- **Config:** a build-time check that `/etc/pixi/config.toml` parses (`pixi config list`) and, if §4 needed it, holds
  the sharding setting. With `SANDBOX_CONDA_MIRROR` set, the setting is also rendered for the mirror's URL.
- **PATH:** FIX-298's `PATH` assertions move again (`pty_shell.rs`, `hosted.rs`'s `IMAGE_PATH`, and anything
  `rg -F '/workspace/.local/bin:'` finds).
- **Optional live check, on a throwaway kind cluster only (never the `aurora-dev` stack, `:4000` or its
  database):**
  - `pixi exec --spec go-yq -- yq --version` and `pixi global install imagemagick` through the gateway.
  - Record the hosts contacted and the image's compressed size before and after.
  - Record the paths fetched: the gateway log shows `repodata_shards.msgpack.zst` and shard files, never
    `repodata.json`.
  - If you don't run it, write "not run".

## Tests move with the change
Every test or fixture that pins the `PATH`, the instruction file, or the image's size budget is updated in the
same commit. Never skip, mark `fixme`, delete or loosen a test. If an image-size guard fails, report the
measured compressed sizes and raise it in the same commit.

## Placement
- **One Sonnet lane, size M, after FIX-298 merges** (both edit the `PATH` files).
- **Files:** `images/sandbox/Containerfile`, `images/sandbox/etc/pixi/config.toml` (new),
  `etc/profile.d/aurora.sh`, `etc/zshenv`, `services/aurora-supervisor/src/{spawn.rs,instructions.rs}`, the
  supervisor's boot path (prune), `tests/it/pty_shell.rs`, `tests/e2e-hosted/tests/hosted.rs`, and docs
  `11`, `12`, `15`, `17`.
- **Security (SEC-174):**
  - The supervisor deletes inside an agent-controlled tree.
  - The sandbox gains a package source.
  - An Opus review runs before the merge. It checks the prune (no link followed, no blocking read, no
    deletion outside the home), that the channel and `exclude-newer` defaults are described as defaults and
    not controls, and that the agent's posture is unchanged (no root, read-only root, gateway egress only).
- **Lane gates:** rebuild the sandbox image through `heavy.sh`; `scripts/dev/test.sh <lane> rust -p
  aurora-supervisor` filtered to the prune, spawn, instructions and PTY tests; the new hosted scenario locally
  if the image builds; `cargo fmt --check`, `scripts/guards/run-all.sh` and crate clippy. CI runs the full
  hosted e2e.

## Completion report
(lane: pixi version and digests; config as shipped; hosts contacted; image size before/after; the prune's
code path and its tests; the instruction text as shipped; live check result or "not run".)
