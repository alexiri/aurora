# P2-DOCS-2: Operator docs for the phase-1 and phase-2 configuration (AUDIT-002 F-7, F-3)

## Problem
Exit criterion 11 asks that operator docs cover the phase-1/2 configuration. AUDIT-002 found that `docs/operator/*` has no page or section for several topics; they live only in the design document `docs/15` (F-7). Separately, the chart's default images `ghcr.io/alexiri/aurora-*` aren't pullable (no CI publish while billing is down, and ghcr packages are private by default), so an operator following `install-helm.md` literally cannot install (F-3).

## Scope
Write operator documentation: task-oriented, accurate to the code on master, and with every key cross-checked against `docs/15` and `crates/aurora-config`. **No behaviour or config changes.**

1. **F-7: one section per topic**, in existing pages where they fit, or new pages under `docs/operator/` linked from `docs/operator/README.md`. Each section covers what it does, the keys with defaults, a minimal example, and how to verify it. Topics:
   - the TLS proxy listener;
   - OIDC-only sign-in and the dev IdP (dev only, refused on non-loopback);
   - open-lane posture (`enabled` / `allowlist_only` / `disabled`, the browser capability);
   - BYO posture and source allow-lists (incl. `gateway.trusted_proxies` and PROXY protocol);
   - retention and attachment quota (`conversation_days_max`, org settings, the sweep);
   - API rate limits (classes, multiplier, overrides);
   - signing-key and internal-secret keyring rotation;
   - approval timeouts and e-mail (`approvals.*`, the mailer, recipients);
   - app instances (registration and admin flow, D-74);
   - the built-in catalogue (what ships, signing, provenance in `plugins/builtin/ICONS.md`);
   - the database lane (TLS, `psql service=`, classes and policy);
   - freeze;
   - sandbox resources (`runner.sandbox_*`) and private sandbox registries (FIX-013).

   Link to the design docs for the full normative text instead of copying it.
2. **F-3: images.**
   - In `install-helm.md`, state plainly that the `ghcr.io/alexiri` defaults exist only after a CI publish, and that the packages must be made public (or pulled with a secret).
   - Add a "build and load the images yourself" path for kind and for a real cluster with a private registry, modelled on what `scripts/e2e/kind.sh` and `deploy/dev/kind-dev.sh` do.
   - Make "public" wording accurate everywhere it appears.
3. Keep `docs/15` as the source of truth; operator docs summarise and link.

## Constraints
- **Owns:** `docs/operator/**` only. No changes to `docs/0*`–`docs/2*`, code, the chart or scripts. If you find a doc/code mismatch, report it rather than changing either.
- Real product names everywhere (SEC-33 restricts core code only).

## Acceptance criteria
- [ ] Every topic above has a section, and `docs/operator/README.md` links to it.
- [ ] Every config key you name exists in `crates/aurora-config/src/raw.rs` (list any you couldn't confirm).
- [ ] `bash scripts/plugins/check-links.sh` or the repo's doc-link check, if it covers `docs/` (say which you ran), and `bash scripts/guards/run-all.sh`.
