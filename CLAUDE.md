# Cost-efficient Claude Code orchestration — Rust + Next.js

## Mission
Deliver the application described by `docs/MASTER_SPEC.md` accurately while using the least expensive model and smallest useful context for each task. Correctness and security take priority over saving usage.

## Source of truth
- `docs/MASTER_SPEC.md` (and other mentioned documents) is the product source of truth.
- `docs/90-decisions.md` records approved technical decisions and interfaces.
- `docs/FEATURES.md` is the feature inventory.
- `docs/tasks/` contains bounded implementation briefs.
- `docs/PROGRESS.md` tracks status, dependencies, and verification.

Do not invent requirements. If the master specification is absent, ask the user to add it before planning implementation.

## Stack boundaries
- Rust owns domain/business rules, authorization, invariant validation, persistence, transactions, and backend integrations. Discover the actual edition, workspace, framework, async runtime, DB library, and error conventions; never assume Axum/Actix/Tokio/SQLx/Diesel.
- Next.js owns presentation, interaction, navigation, and UI state. Discover installed version and App Router/Pages Router structure. Respect Server/Client Component boundaries, existing TypeScript strictness, styling, and test conventions.
- Every dashboard screen, existing or new, follows the visual identity in `docs/14-web-dashboard.md` §Visual design: tokens only, the shared UI primitives and page patterns, both themes, no horizontal scroll at 390 px. No one-off styling where a primitive or pattern exists; add missing ones to the shared set.
- Frontend validation improves UX but backend validation remains authoritative.
- `docs/05-control-plane-api.md` (et al) is canonical for routes, methods, auth, request/response schemas, errors, pagination, nullability, IDs, and timestamps. Define/update contracts before parallel backend/frontend work. Do not let agents independently invent the same API shape.

## Model allocation
- Opus main session: architecture, specification reconciliation, contract decisions, difficult system-wide debugging, orchestration and final audit.
- Sonnet: substantial Rust or Next.js implementation, multi-file features, integration, non-trivial debugging.
- Haiku: bounded low-risk coding, targeted exploration, simple helpers/components, documentation, straightforward tests.
Choose the least expensive model likely to succeed; escalate when complexity warrants it.

## Delegation and context
1. Decompose requirements into small, independently verifiable tasks.
2. Give each worker one task brief, relevant architecture/API excerpts, exact paths, constraints and acceptance criteria.
3. Never send the full master spec unless truly necessary. Aim for self-contained briefs around 500–1,500 words or less.
4. Use at most 3 implementation agents concurrently by default.
5. Keep file ownership disjoint. Do not parallelize edits to the same files, migrations, lockfiles, generated files, shared contracts, or central configuration.
6. Use `haiku-explorer` for narrow read-only discovery, `haiku-worker` for small isolated work, `haiku-tester` for tests only, `rust-developer` for Rust, `nextjs-developer` for Next.js, and `sonnet-integration-reviewer` for read-only cross-stack review.
7. Main Opus session owns task creation, integration, review, progress updates, and dependency scheduling.
8. Avoid redundant agents, repeated full-context scans, unnecessary broad builds, and repeated failed approaches. Subagents consume usage too; delegation is not free.

## Task lifecycle
Plan -> define/update API contract -> write bounded brief -> discover if needed -> implement -> targeted verify -> integrate -> update progress and requirement traceability.

## Quality gates
- Rust: use repo-appropriate `cargo fmt --check`, targeted `cargo test`, and clippy checks.
- Next.js: use repo-appropriate tests, TypeScript checks, lint/format, and production build.
- Cross-stack: verify schemas, error handling, auth/session/cookies/CORS where relevant, and important end-to-end journeys.
- Never claim a check passed unless it ran and passed. Do not weaken security, validation, privacy, or data integrity to save usage.
- Ask before destructive commands, data-loss-risk migrations, production operations, or consequential irreversible choices.

## Escalation
Haiku -> Sonnet for ambiguity, multi-file work, cross-module changes, or non-obvious test failures. Sonnet -> Opus for architecture, security/data-integrity choices, conflicting requirements, or system-wide debugging.

## Reporting
Report completed tasks, files changed, exact verification results, blockers/decisions, and next ready tasks. A task is Done only when its acceptance criteria have evidence.
