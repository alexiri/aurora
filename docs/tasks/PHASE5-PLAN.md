# Phase 5 — lane plan (coordinator, 2026-11-14)

Owner, 2026-11-14: phase 5 starts when AUDIT-004 is merged; same working mode as phase 4 — **three lanes**, each in its own worktree, several related tasks per lane with **targeted tests between tasks and full suites at batch ends** (`docs/tasks/LANE_RULES.md`); the coordinator reviews, sends back, merges, pushes. Briefs are normative; this file only orders them and allocates numbers. Status lives in `docs/PROGRESS.md`.

## Wave 1 (starts when AUDIT-004 is merged)

| Lane | Chain (batch gates at `‖`) | Why together | Hot files it alone owns while running |
|---|---|---|---|
| `p5-apps` | FIX-147 → FIX-133 → FIX-136 → FIX-141 ‖ FIX-144 → FIX-145 ‖ FIX-149 ‖ FIX-134 → FIX-135 ‖ (FIX-149 added 2026-11-14: same connect-window files) | every one touches the connect window, the registry manifest or the gateway's per-app decision; the briefs order them (147 before 133/136; 136 before 141; 144 before 145; 134 before 135) | `connect-view.tsx`, `aurora-registry` manifest/linter, gateway `resolve/apps/decide.rs`, `plugins/builtin/{github,indico,sentry,atlassian}`, `connect/**` |
| `p5-agent` | P5-AGENT-1 → FIX-137 → P5-AGENT-2 → FIX-138 ‖ P5-CHAT-1 → P5-CHAT-4 A (+C) ‖ | the agent page and the chat transcript: one owner for `web/package.json`/lockfile (FIX-137 first, then P5-AGENT-2), the agent page (P5-AGENT-2 before FIX-138), then `domains/conversations` and its migrations | `web/package.json` + `pnpm-lock.yaml`, `agent-page.tsx`, `components/chat/**`, `domains/conversations/**`, `repo/{turns,transcript}.rs` |
| `p5-platform` | P5-ADMDEL → OPS-021 → FIX-142 ‖ P5-ASRC (rebase and finish the paused WIP `120db005` on `alexiri/p5-asrc`) ‖ P5-CERN-ADDONS ‖ | instance-admin/ops work, the channels sanitizer, then the addon-repository feature and its first consumer | `domains/{tenancy,placement}/**`, admin org pages, `aurora-core/sanitize.rs`, `aurora-channels/content.rs`, `connectors/{mattermost,fake}` render, addon-sources domain |

## Wave 2 (as wave-1 lanes free up)
- **FIX-146** (request tracing, L, parts A → B/C/D → E): after P5-CHAT-1 (both touch the `turns` insert), after FIX-145 A (gateway resolver), and when no lane holds `web/package.json` (part E) — takes the first free lane after `p5-agent`'s first batch.
- **P5-CHAT-2 → P5-CHAT-3 → P5-CHAT-4 B**: after the owner has reviewed the P5-CHAT-0 boards; in `p5-agent` (same files).
- **SPEC-SETTINGS → SET-1 → FIX-124 → SET-2..SET-5**: after the spec batch (coordinator) and the owner's answers to its open classification questions.
- The items with no brief yet (below), in the order the owner gives.

### Wave 2 lanes (started 2026-11-15, after wave 1 merged; same three worktrees, branches reset to `master`)

| Lane | Chain (batch gates at `‖`) | Why together | Hot files it alone owns while running |
|---|---|---|---|
| `p5-apps` → settings | SET-1 → FIX-124 ‖ SET-2 → SET-3 ‖ SET-4 → SET-5 ‖ P4-LLM-6 (after SET-3 and P4-LLM-1 are merged) | one settings framework and its consumers, in brief order | `aurora-config` settings registry, `domains/settings/**`, Admin › Settings pages, `aurora-secrets` store admin |
| `p5-agent` | P5-CHAT-4 B ‖ P4-LLM-1 → P4-LLM-2 ‖ P4-LLM-3 → P4-LLM-4 → P4-LLM-5 ‖ | finishes the chat chain, then the LLM provider chain (runner harness config + gateway mint) | `components/chat/**` search, `aurora-runner` provider/harness config, gateway `llm/**` |
| `p5-platform` | P5-CERN-ADDONS external repo ‖ FIX-146 A → B/C/D → E ‖ P4-SSH-1 → P4-SSH-2 ‖ | the addon repo's release, then cross-cutting tracing, then SSH (both need `web/package.json`) | `web/package.json` + `pnpm-lock.yaml` (this wave), `/home/alex/agent-platform/aurora-addons-cern`, tracing middleware, `aurora-terminator` |

### Wave 3 queue (placed 2026-11-15; each lane continues after its wave-2 chain, merging master first)

| Lane | Queue after its wave-2 chain | Why here |
|---|---|---|
| `p5-apps` | P5-GUIDE-1 → {2 ∥ 4 ∥ 5} → 3 → 6 → 7 ‖ P5-ICS → P5-CALDAV ‖ | the registry, package format and built-in packages are this lane's ground; P5-GUIDE-1 and P5-ICS both change the registry, so they run in one lane, one after the other |
| `p5-agent` | FIX-150 → P4-NARR → P4-UTOK ‖ P4-EVAL-1 → {2 ∥ 4} → 3 ‖ | channels and messaging tools (FIX-150, `send_as_me`), then evals, whose judge builds on the LLM chain's providers |
| `p5-platform` | P4-011B-1 → {2 ∥ 3} → 4 → 5 ‖ P4-S3 ‖ P5-CALINV ‖ | gateway holds and Web Push next to the SSH and tracing work; the blob store; the mailer-only calendar invite |

**Ordering constraint:** P5-GUIDE-7 must not run beside a lane editing `services/aurora-api/src/domains/tools/` (FIX-150, P4-UTOK and P4-EVAL-2 do), so `p5-apps` holds P5-GUIDE-7 until those three are merged. If one lane finishes much earlier, the coordinator moves the head of another lane's queue to it. **FIX-150** (S–M, `channels/messaging.rs` + `tools/messaging.rs` only, disjoint from the wave-2 hot files) may be inserted at any lane's next batch gate; not concurrent with P4-UTOK/P4-NARR.

## Coordinator (Opus) work, in order
1. **Spec step 0** for FIX-142, FIX-145, FIX-146 (unblocks wave 1/2 lanes).
2. **SPEC-SETTINGS** spec batch → SET-1..SET-5 briefs (owner answers its open classification questions).
3. **P5-CHAT-0** design boards + `docs/14` (owner reviews before P5-CHAT-2).
4. Specs/briefs for the rows with none, in the owner's order (D-235(c)): **P4-LLM, P4-SSH-1/2, P4-011B** first; then P4-S3, P4-EVAL, P4-NARR, P4-UTOK (P5-SLACK moved to phase 6, D-285).

Done 2026-11-14: specs merged for SPEC-SETTINGS (SET-1..SET-5), P4-SSH (P4-SSH-1/2), P4-LLM (P4-LLM-1..6), P4-011B (P4-011B-1..5); owner answers D-235, D-268. Still to spec: P5-SLACK, P4-S3, P4-EVAL, P4-NARR, P4-UTOK; P5-CHAT-0 boards.
Done 2026-11-14: step 0 of FIX-142 (46bbf084), FIX-145 (f8fb415b), FIX-146 (c8aa4cb5). SPEC-SETTINGS' owner questions answered (D-235(a)/(b)); P5-CERN-ADDONS uses the gateway lane (D-235(d)).
5. Perf budgets on an idle machine (carried from phase 4).

## Number allocation (lanes use exactly these; take the next one in the block)
| Lane / owner | Migrations | Decisions | Contracts |
|---|---|---|---|
| `p5-apps` | `20261201000160` … `000169` (FIX-145 keeps its `000140` block) | D-240 … D-244 | C-740 … C-749 (FIX-144 C-730, FIX-145 C-731 as briefed) |
| `p5-agent` | `20261201000170` … `000179` | D-245 … D-249 | C-750 … C-759 (C-700, C-725..C-728 as briefed) |
| `p5-platform` | `20261201000180` … `000189` | D-250 … D-254 (P5-ASRC's trust-model decision is **D-250**, not D-159 — taken) | C-590 … C-609 (P5-ASRC, free), C-760 … C-769 (C-733 as briefed) |
| FIX-146 (wave 2) | `20261201000150` … `000159` (as briefed) | D-232 (written), more from D-255 … D-259 | C-732 (as briefed) |
| coordinator specs | — | D-235 … D-239 | C-770 … C-799 |

Other sessions writing phase-5 briefs: take numbers **outside** these blocks (next free after D-259, C-800, migration `000190`) and note them in PROGRESS.
