# REVIEW-5 findings — lifecycle (saved by the coordinator from the reviewer's hand-back)

## LIFE-1 (medium, concurrency, CONFIRMED) — readiness sweep fails turns from a stale snapshot even when the fence says the sandbox became ready
- `services/aurora-api/src/domains/hosted/readiness.rs:237-246` (snapshot), `:249-287`, `:259-268` (`fail_attempt` None ignored), `:277-279` (`lifecycle::fail` unconditional); `hosted/lifecycle.rs:52` (accepts queued|dispatched|running); `crates/aurora-db/src/repo/sandboxes.rs:558-566`.
- Fix: skip `lifecycle::fail` when `fail_attempt` returned None, or re-read under lock and fail only still-`queued` turns.

## LIFE-2 (medium, spec-defect + code) — a start with no queued turn (preview/terminal/schedule wake) has no deadline; runner gone → `starting` forever, `runner_id` pinned
- `sandboxes.rs:36-46`, `:337`, `:33`; `readiness.rs:237-246`, `:95-104`; `sandboxes.rs:492-507`; nothing sets `runner_id = NULL`. C-580, C-584.
- Fix: amend C-580 (every `starting` attempt bounded from its claim); sweep pass over `state='starting' AND lifecycle_lease_until < now()`.

## LIFE-3 (medium, PLAUSIBLE) — late runner `failed` on `stopping` after the API's give-up double-counts `start_failures` and overwrites the specific reason
- `sandboxes.rs:597-607`, `:939-950`, `:959`. C-583.

## LIFE-4 (medium, CONFIRMED) — one transient Kubernetes API error during a start fails the start and deletes the pod
- `services/aurora-runner/src/backends/kubernetes/mod.rs:604-607`, `:661-664`, `:575`, `:586-587`, `:744`; `backend.rs:67-76`.

## LIFE-5 (medium, PLAUSIBLE, SEC-66) — Codex `config.toml` lives in agent-writable `/workspace/.codex`, written once per session; agent can change `base_url` / MCP command between turns
- `services/aurora-supervisor/src/adapters/codex/mod.rs:136-140`, `:300-327`, `:74-78`; `config.rs:110-146`, `:1-5`, `:161-182`.
- Fix: re-render before each spawn and/or CODEX_HOME in a supervisor-owned dir outside /workspace.

## LIFE-6 (low) — with no live runner, a `failed` sandbox's specific failure code is replaced by `runner_unavailable`
- `readiness.rs:93-95` vs `:135-165`, `:80-89`.

## LIFE-7 (low, info disclosure) — runner cluster IP/service, image ref and kubelet event text reach every workspace member
- `kubernetes/mod.rs:800-807`; `executor.rs:461-466`; `readiness.rs:39-59`, `:65-71`; `hosted/routes.rs:128-150`, `:165-187`; `conversations/service.rs:40-46`.

## LIFE-8 (low) — Codex session refs from the stream stored unvalidated; purge matches by suffix
- `codex/mod.rs:279-283`, `:81-85`, `:189-203`; `harness.rs:238-243`.

## LIFE-9 (low) — Codex stdout reader has no line-length bound
- `codex/launcher.rs:80-84`; `codex/stream.rs:77`.

## LIFE-10 (low, PLAUSIBLE) — `unbound_home` does not classify a `Lost` PVC
- `kubernetes/mod.rs:372-383`.

## LIFE-11 (medium, test-gap) — lifecycle tests are sequential only; k8s mock never injects 5xx/timeouts
- `services/aurora-api/tests/it/fix_029.rs`; `runner_plane.rs:335-371`; `services/aurora-runner/tests/it/k8s_mock.rs`.

## Not covered
- `runners_admin/**`; stop routes / freeze vs restart; harness switch vs turn-creation race; `jobs.rs`; podman backend; Codex classify_failure; pod event ordering; operator docs.
