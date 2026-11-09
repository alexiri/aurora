# SPEC-P4a: Normative spec for the phase-4 groundwork batch (P4-B1 + P4-HAR + P4-GRANTS)

## Goal
The owner selected every phase-4 item, in the proposed batch order (Q10, 2026-11-09: "continue implementing everything that's left"). Phase-4 designs in `docs/21` are design-level only: no tables, routes, config keys, limits or SEC invariants. Write the **normative spec** for the first batch so implementation can be briefed from it. Specification only; no code.

Items (one commit each, prefix `SPEC-P4a: <item>`):
1. **P4-CNT — shared rate-limit counters.** A `CounterStore` seam replacing the per-replica in-memory counters of the gateway rate limiter and the API limiter (D-05 documents the N× approximation it removes). Backends: **in-memory** (today's behaviour, the default, single replica), **Postgres**, and **Redis** (optional, behind config). Specify: the trait semantics (fixed/sliding window, atomic increment-and-check, TTL), which limiters use it, config keys, what happens when the backend is unreachable (fail-open or fail-closed **per limiter**, with the reason; security-relevant limiters such as sign-in and BYO admission must not fail open), metrics, the Postgres table if any, Helm values, and tests.
2. **P4-S3 — S3-compatible `BlobStore`.** A second backend beside the filesystem store for attachments (D-54). Specify: config (endpoint, bucket, region, path-style, credentials as secret references, never inline), server-side encryption expectations, object key layout, size limits and streaming, deletion and retention interplay, what the API does when the store is unavailable, migration from filesystem (or explicitly none), and tests (a MinIO-style container in e2e).
3. **P4-LLM — additional LLM providers:** OpenAI, Azure OpenAI, AWS Bedrock, Google Vertex, generic OpenAI-compatible. Build on the provider registry (D-16, `08 §11`, `crates/aurora-llm`). Specify per provider: dialect(s), auth (keys as secret references; Bedrock SigV4 and Vertex service accounts never reach a sandbox, same as today's keys), host allow-listing and the Default Rule LLM-host carve-out (`07`, RFE-023), streaming, model listing, error mapping, governance and usage accounting, config keys, and tests (mock upstreams).
4. **P4-HAR — harness adapters** declaring `openai_*` dialects (e.g. Codex). Specify the adapter contract in `12`, how the supervisor selects a harness, which dialects each harness accepts, the sandbox image impact, and tests. P4-HAR is implemented after P4-LLM.
5. **P4-GRANTS — "custom grants only"** app-level manifest option (D-84): for apps whose token is broad (Indico `full:everything`), forbid `full` grants or always block untooled requests. Specify the manifest field, registry validation, policy evaluation, dashboard behaviour, and how it composes with existing grants. Also decide **Q8, parameter-level tool matching** (so AWS STS/EC2 `Describe*` become read tools): either specify it as a manifest-contract extension with its matching rules, or record why it stays deferred. Ask through Orca `ask` if Q8 changes the manifest contract in a way existing packages must follow.

## Rules (as SPEC-RFE-P3)
- **Phase-tag** every new or changed rule "(phase 4, P4-xxx)". Where a phase-0..3 rule is superseded, keep it readable and state the new rule as superseding it from phase 4.
- **Update every owning doc**: 01 trust boundaries, 02 §Limits, 03 data model, 04 permission matrix, 05 routes/jobs, 06 gateway, 07 policy, 08 secrets/LLM, 09 registry/manifest, 11/12 runner/supervisor, 14 dashboard, 15 config keys and Helm, 16 wire protocols, 17 security (SEC invariants + residual risks), 18 testing (what proves each item), 21 (mark the items as specified and point to the owning sections).
- **Data model (`03`)**: never add future tables/columns to the machine-checked phase-0 tables; use `## Planned changes (later phases)` with level-4 headings and a `| Planned column | Type | Notes | Phase |` header. Verify the extract is unchanged: `python3 scripts/extract-schema.py --check` and `cargo test -p aurora-db --test it schema`.
- Where a choice is open, pick the **most secure reasonable default**, write it, and list it under "Choices made" in your report. If a choice is consequential (changes a security property, adds required infrastructure, or breaks existing packages/configs), ask through Orca `ask`.
- New config is **optional with today's behaviour as the default**: an existing `aurora.toml` and Helm values must keep working unchanged.

## Numbers reserved for this task
- Decisions **D-119..D-126** in `docs/90-decisions.md` (one per item, plus spares).
- SEC invariants **SEC-62..SEC-68** in `docs/17-security.md` and `docs/tests/security-coverage.md` (status `pending`).
- Contracts **C-400..C-449** are reserved for these items' implementation; do **not** write `docs/API_CONTRACTS.md` entries (they are written at implementation time), but name the routes and shapes in `05`.

## Constraints
- You own `docs/0*.md`, `docs/1*.md`, `docs/21-later-phases.md`, `docs/90-decisions.md` (D-119..D-126 only), `docs/tests/security-coverage.md` (new rows only).
- Don't edit `docs/80-design-review.md`, `docs/API_CONTRACTS.md`, `docs/PROGRESS.md`, `docs/FEATURES.md`, `docs/operator/**`, `docs/audit/**`, or any code. AUDIT-003 runs in parallel and owns `docs/audit/**`.
- Change files only with the Edit/Write tools; never python, sed, awk, perl, `cat >>` or heredocs.

## Acceptance
- One commit per item; `bash scripts/guards/run-all.sh` and `python3 scripts/extract-schema.py --check` pass; `cargo test -p aurora-db --test it schema` passes.
- Report: per item, every doc and section changed; "Choices made"; open questions; the Q8 outcome.
