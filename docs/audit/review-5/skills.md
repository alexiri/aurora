# REVIEW-5 findings — skills (saved by the coordinator from the reviewer's hand-back)

## SKILL-1 (high; code path confirmed, amplification reasoned from serde_yaml 0.9.34 source, not run) — SKILL.md frontmatter YAML alias expansion unbounded (billion laughs) in the API
- `services/aurora-api/src/domains/skill_libraries/content.rs:441` (`serde_yaml::from_str::<serde_yaml::Value>`), `:422` (`parse_skill_md`); `service.rs:546-548`, `:587` (browse parses up to 200 SKILL.md), `:583` (only 64 KiB raw bound); `load` (add/refresh). Cache filled only after the parse → repeats.
- Fix: typed struct (name, description) / reject anchors+aliases before parsing; retire deprecated serde_yaml. Owns content.rs.

## SKILL-2 (medium, PLAUSIBLE on host behaviour; spec-defect) — browse/add accept any full commit id, not only commits reachable from the library ref
- `skill_libraries/service.rs:647-658`, `:606-630`; `skills/library.rs:152-240`; C-571, C-572.
- Options: (a) reachability check, (b) restrict to current head + recorded heads, (c) document.

## SKILL-3 (low) — cold browse: no single-flight, no deadline; `commit_sha` reads bypass the cache (`service.rs:520-565`, `:546-548`, `:647-658`; `routes.rs:204-260`; `content.rs:96` BUSY_RETRIES)
## SKILL-4 (low, concurrency) — refresh pass overwrites a `last_head_sha` cleared by a concurrent add (`repo/skill_libraries.rs:705-713`, `:561-573`; `skills/library.rs:240`; `refresh.rs:236`); C-577, D-154(d)
## SKILL-5 (low, concurrency) — org 4 MiB open-proposal cap check-then-act in refresh (`refresh.rs:239`, `:327-331`, ~`:345`; add path takes `lock_budget` `skills/library.rs:135`)
## SKILL-6 (low) — approval re-validates only hash/name/total size, not file/frontmatter rules (SEC-83 "re-validates") (`content.rs` `finish`; `skills/library.rs:680`, `:456-490`)
## SKILL-7 (low, PLAUSIBLE/spec) — pausing a library doesn't stop approval of pending proposals (`skills/library.rs:627-700`, `:600-611`); C-570, C-574
## SKILL-8 (low) — git-host 429 counts as a refresh failure → 6 h backoff; Retry-After ignored (`content.rs:96-99`; `refresh.rs` `next_in_secs`; `library_fetch.rs:536-541`)
## SKILL-9 (low, docs) — operator doc overstates browse cost (`docs/operator/skill-libraries.md:28`)
## SKILL-10 (low, unwired) — `required_access` parsed, never read (`crates/aurora-registry/src/manifest/model.rs:184-193`)

## Not covered
- git_host fake fidelity; migration constraints/indexes; several repo SQL bodies; gateway refresh.rs internals; frozen-org behaviour; internal-secret rotation for HttpGateway; dashboard rendering of repo text.
