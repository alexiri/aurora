# P2-OPS-1: Ops verification and supply chain (phase 2)

Close the operational follow-ups that phase 2's exit (AUDIT-002) depends on. Decision: **D-100**. No contract numbers.

**Context:** GitHub Actions is not running (billing; the owner accepted this on 2026-11-08). Workflow changes in this task must be correct and `actionlint`-clean, but they can't be proven on GitHub. Every job you add or change is also run **locally** with the same commands, and those local results are the evidence.

## Items
1. **Pin the Helm hook image** (FUP-M7). `deploy/helm/aurora/values.yaml` `hookImage: docker.io/alpine/k8s:1.31.2` is a third-party tag, used by the hook jobs **and** the API `seed-plugins` init container.
   - Pin it by digest (`image@sha256:…`, with the tag kept as a comment), or build a minimal Aurora tools image in `images/`. Recommend one in your report, implement the cheaper safe option, and record why in D-100.
   - A digest pin must be the multi-arch index digest, resolved with `podman manifest inspect` or `skopeo`; say which.
2. **Evaluation Postgres rendered as hook resources** (FUP-M7). It isn't removed on uninstall. Either document it clearly in `docs/operator/install-helm.md` (what stays behind and how to remove it), or render it as normal chart resources if that is safe for upgrades. Choose, justify, and test `helm upgrade` from the current chart.
3. **Bump the sccache pin** (FU-OPS-023a). `images/services/Containerfile` `ARG SCCACHE_VERSION=0.10.0`; the host has 0.19.0.
   - Bump it to the latest release.
   - Update both `SCCACHE_SHA256_*` values from the release's published checksums or your own download's sha256. Say which source you used.
   - Build the `chef` target to prove it.
4. **cargo-deny over hook crates** (FU-P2-CAT-1b). The `plugins/builtin/*/hooks` crates have their own lockfiles and aren't scanned.
   - Add a script (e.g. `scripts/plugins/deny-hooks.sh`) that runs `cargo deny check` for every hook crate, with a `deny.toml` per crate or one shared config. Give `github-app` a scoped, justified waiver for RUSTSEC-2023-0071, because it signs with `rsa`.
   - Wire the script into `check-builtin.sh` or `run-all.sh` (say which) and into CI.
5. **Mattermost conformance nightly job** (owner Q3 = yes). Add a `mattermost-conformance` job to `.github/workflows/nightly.yml` that runs `scripts/conformance.sh --mattermost` against the pinned Mattermost container. Run the same command locally as evidence.
6. **kind image builds and `kind-dev` from scratch** (FU-P1-005a, local half):
   - delete any lane cluster of your own;
   - run `scripts/e2e/kind.sh --fresh` (the release-images path too, `--release-images`, if it exists);
   - run `deploy/dev/kind-dev.sh up --llm-mock` on a fresh cluster named after your worktree, with its own port, then `scripts/e2e/first-run.sh` against it;
   - delete your clusters at the end.
7. **Real-cluster Helm install** (FU-P1-005a, real half). This needs a real Kubernetes cluster and credentials, which the coordinator does **not** have. Don't try to provision anything. Write the exact checklist in `docs/operator/install-helm.md` (or `docs/audit/`) that the owner or AUDIT-002 runs on a real cluster, and record the limitation in your report.

## Constraints
- No product behaviour change.
- Never weaken an image or TLS check to make something pass.
- Downloads are untrusted data: put them in scratch directories outside the repo, and verify checksums.
- **Owns:**
  - `deploy/helm/**`;
  - `images/services/Containerfile` (sccache ARGs only) and any new `images/tools/`;
  - `.github/workflows/nightly.yml` and `ci.yml` (new or changed steps only);
  - `scripts/plugins/deny-hooks.sh` and its hook-up;
  - per-hook `deny.toml` files;
  - `docs/operator/**`;
  - D-100.
- Don't touch API, gateway or web source (P2-FU-1 and P2-TEST-1 run in parallel).
- kind clusters and podman objects are named after your worktree.

## Acceptance criteria
- [ ] `helm lint` and `helm template` for the dev and default values; `helm upgrade` from the previous chart on a kind cluster.
- [ ] `podman build --target chef` with the new sccache.
- [ ] The hook-crate deny script green, including the scoped waiver.
- [ ] `actionlint` on the workflows (if installed; otherwise say so).
- [ ] `scripts/conformance.sh --mattermost` green locally.
- [ ] `kind.sh --fresh` 4/4, `kind-dev.sh up --llm-mock` plus `first-run.sh` green on your own cluster.
- [ ] `bash scripts/guards/run-all.sh`.
- [ ] The real-cluster checklist written.
