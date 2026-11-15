# REVIEW-5 findings — ops (saved by the coordinator from the reviewer's hand-back)

## OPS-1 (high) — gatewayApi + relay: preview wildcard put on `rules[].hostnames` (no such field in HTTPRouteRule) → host-less `/` catch-all to relay shadows the dashboard, or the render is rejected
- `deploy/helm/aurora/templates/routing.yaml:80-88` (relay rule), `:112-115` (web `/`), `:73-74` (spec hostnames); guard `scripts/guards/edge-routes.sh:36-37`, `:121-124` assumes rule-level hostnames valid.
- Spec: docs/15 §Networking — wildcard host is "a second host, never a path of server.external_url … Without it nothing about the edge changes."
- Fix: own HTTPRoute `<full>-previews` with `spec.hostnames: ["*.<domain>"]`; drop rule-level block and spec-level wildcard; guard rejects any `rules[].hostnames`, checks dashboard host `/` → web.

## OPS-2 (medium, test-gap) — CI runs gateway e2e without FRONTDOOR=1; front-door tier skips silently; docs/18:16 says "CI-only strict front doors"
- `.github/workflows/ci.yml:188`; `scripts/e2e/gateway.sh:128`; `tests/e2e-gateway/tests/frontdoor.rs:30-37`, `:214-225`.

## OPS-3 (medium, test-gap) — podman-backend sandbox hardening tests never run in CI (AURORA_TEST_PODMAN gate; early return passes)
- `services/aurora-runner/tests/it/podman.rs:25-27`, `:133-137`, `:264-268`; `ci.yml:145-170`, `:238`; `scripts/e2e/hosted.sh:92-95`.

## OPS-4 (medium) — sandbox image labels `browser` capability even when Chromium failed to install
- `images/sandbox/Containerfile:58` (`|| echo "chromium unavailable"`), `:75` (label unconditional); `scripts/images/sandbox-base-smoke.sh:139-153`.

## OPS-5 (medium, test-gap) — quick gate can pass while affected tests never run
- `scripts/dev/quick-gate.sh:263-267`, `:305-309` (migration → only aurora-db + aurora-api; gateway/supervisor use TestDb from `aurora_db::MIGRATOR`, `crates/aurora-testkit/src/template.rs:99`), `:294` (`*) ;;` plugins/, scripts/, deploy/, images/, .github/, justfile → nothing; `services/aurora-api/tests/it/connect.rs:182` uses plugins/fixtures/dist), `:374`, `:433-466` (guards never run in default scope).

## OPS-6 (low) — SCIM / terminal / previews switches can be half-enabled without render failure
- `configmap.yaml:66-70`, `:29-34`, `:49-54`, `:64`; `values.yaml:160-161`, `:343-345`; `routing.yaml:38-44`, `:89-97`; `networkpolicy.yaml:180-189`, `:206`.

## OPS-7 (low) — runner k8s API egress defaults 0.0.0.0/0 on 443/6443 while the hook derives the API server
- `networkpolicy.yaml:194-205`; `hooks.yaml:78-91`; `values.yaml:397-403`.

## OPS-8 (low) — single-replica runner/relay/connector host get PDB minAvailable 1 (drain blocked)
- `_helpers.tpl:242-253`; `values.yaml:303-305`, `:221`, `:265`, `:249`; `docs/operator/install-helm.md:379`.

## OPS-9 (low, test-gap) — sandbox-network guard checks only the sandboxes namespace; core-side ingress from sandboxes unchecked
- `scripts/guards/sandbox-network.sh:25-40`; `networkpolicy.yaml:124-131`, `:175-176`.

## OPS-10 (low) — leaked `aurora_test_<uuid>` databases never reaped
- `crates/aurora-testkit/src/lib.rs:182-205`, `:77`; `justfile:91-93`; `template.rs:305-335`.

## OPS-11 (low, PLAUSIBLE) — template reaper drops other migration sets by age WITH FORCE; clone in another worktree can fail
- `template.rs:310-335`, `:340-363`, `:365-380`.

## OPS-12 (low) — hook ServiceAccount can read every Secret in the release namespace (no resourceNames)
- `hooks.yaml:22-35`, `:47`, `:140`, `:225`.

## OPS-13 (low, PLAUSIBLE) — secrets hook silently regenerates encryption keys if the generated Secret was deleted
- `hooks.yaml:155`, `:163-166`, `:121-127`.

## OPS-14 (low, docs) — docs/15:934-935 names non-existent `relay.networkPolicy.runnerEgressTo` (FIX-074 F-5 covers this clause)

## OPS-15 (low) — sandbox image hardening leftovers: rpm package/DB remain (`Containerfile:45`, `:63`), `pip3 install uv` unpinned (`:61`), claude-code npm without lockfile (`:87-88`)

## OPS-16 (low, PLAUSIBLE) — hook-policies guard exempts migrate; `-migrate` policy is a regular resource (upgrade from a chart predating it default-denies the migrate Job)
- `scripts/guards/hook-policies.sh:14-16`; `networkpolicy.yaml:326-334`; `hooks.yaml:257-258`.

## Not covered
- bodies of kind.sh / gateway.sh; most guards beyond vacuous-pass grep; deploy/helm/cases, deploy/kind, deploy/dev; per-component env; real-cluster behaviour; sccache key correctness; docs/operator beyond spot checks; upgrade skew; e2e test bodies.
