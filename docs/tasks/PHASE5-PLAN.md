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
| `p5-apps` → settings | SET-1 → FIX-124 ‖ SET-2 → SET-3 ‖ SET-4 → SET-5 ‖ P4-LLM-6 (after SET-3 and P4-LLM-1 are merged) → P5-MODELS-3 (needs P5-MODELS-1 merged) | one settings framework and its consumers, in brief order | `aurora-config` settings registry, `domains/settings/**`, Admin › Settings pages, `aurora-secrets` store admin |
| `p5-agent` | P5-CHAT-4 B ‖ P4-LLM-1 → P4-LLM-2 ‖ P4-LLM-3 → P4-LLM-4 → P4-LLM-5 ‖ P5-MODELS-1 → P5-MODELS-2 (same adapter code) ‖ | finishes the chat chain, then the LLM provider chain (runner harness config + gateway mint) | `components/chat/**` search, `aurora-runner` provider/harness config, gateway `llm/**` |
| `p5-platform` | P5-CERN-ADDONS external repo ‖ FIX-153 → FIX-154 (owner 2026-11-15: the CERN Phonebook connect dead ends) ‖ FIX-146 A → B/C/D → E ‖ P5-FILES-1 ‖ P5-FILES-2 → P5-FILES-3 → P5-FILES-4 ‖ FIX-156 → FIX-155 (terminal; same supervisor/pty files) ‖ P4-SSH-1 → P4-SSH-2 ‖ (P5-FILES placed 2026-11-15, owner D-316: before P4-SSH, whose Part B builds on FILES-1's `exec.*` engine; FILES-1 touches `aurora-runner` `stream_listener.rs`/`api.rs`/`channel/**`, disjoint from `p5-agent`'s runner provider/harness config — merge master before starting it) | the addon repo's release, then cross-cutting tracing, then the Files page and SSH (all need `web/package.json`; FILES-3 adds CodeMirror) | `web/package.json` + `pnpm-lock.yaml` (this wave), `/home/alex/agent-platform/aurora-addons-cern`, tracing middleware, `aurora-terminator` |

### Wave 3 queue (placed 2026-11-15; each lane continues after its wave-2 chain, merging master first)

| Lane | Queue after its wave-2 chain | Why here |
|---|---|---|
| `p5-apps` | P5-GUIDE-1 → {2 ∥ 4 ∥ 5} → 3 → FIX-151 → 6 → 7 ‖ P5-ICS → P5-CALDAV ‖ | the registry, package format and built-in packages are this lane's ground; P5-GUIDE-1 and P5-ICS both change the registry, so they run in one lane, one after the other |
| `p5-agent` | FIX-150 → FIX-158 → P4-NARR → P4-UTOK ‖ P4-EVAL-1 → {2 ∥ 4} → 3 ‖ | channels and messaging tools (FIX-150, `send_as_me`), then evals, whose judge builds on the LLM chain's providers |
| `p5-platform` | P4-011B-1 → {2 ∥ 3} → 4 → 5 ‖ P5-CALINV ‖ | gateway holds and Web Push next to the SSH and tracing work; the mailer-only calendar invite (P4-S3, the blob store, moved to phase 6, D-318) |

**Ordering constraint:** P5-GUIDE-7 must not run beside a lane editing `services/aurora-api/src/domains/tools/` (FIX-150, P4-UTOK and P4-EVAL-2 do), so `p5-apps` holds P5-GUIDE-7 until those three are merged. If one lane finishes much earlier, the coordinator moves the head of another lane's queue to it. **FIX-150** (S–M, `channels/messaging.rs` + `tools/messaging.rs` only, disjoint from the wave-2 hot files) may be inserted at any lane's next batch gate; not concurrent with P4-UTOK/P4-NARR.

### P5-UX / P5-IA placement (placed 2026-11-16, after the owner merged the spec, 07dd2822)

The twenty briefs run in four fixed waves (D-352, D-353; PROGRESS §Execution order). A brief starts only when everything in its "Depends on" cell is merged. They go **ahead of** the wave-3 queue above, except where a lane would otherwise wait. Design reference for every web brief: the canvas https://claude.ai/artifact/3UwmUgrhh6MLCsfqdhTFmQ (D-354); where a board and the spec disagree, the spec wins and the lane reports the board.

| Lane | Order (batch gates at `‖`) | Why here |
|---|---|---|
| `p5-agent` → Rust lane | P4-LLM-4 + P4-LLM-5 merge ‖ **FIX-159** (high priority, ships alone) ‖ **P5-UX-2** ‖ **P5-UX-13** ‖ P5-MODELS-1 → P5-MODELS-2 ‖ its wave-3 queue | the three `crates/aurora-registry` tasks in one lane, one after the other (FIX-159 → P5-UX-2 → P5-UX-13); P5-MODELS moves behind them |
| `p5-apps` → web lane | SET-5 ‖ **P5-IA-1** (wave 0; first web task; migration `20261201000244`; merged 4e11389a) ‖ **P5-IA-5** (wave 2, its only dependency P5-IA-1; runs while P5-UX-1 waits for P5-UX-2) ‖ **P5-UX-1** (when P5-UX-2 is merged) ‖ **P5-IA-2** ‖ wave 2: P5-UX-3 → P5-UX-9 → P5-IA-3, P5-IA-4, P5-IA-5 (placed when wave 1 ends) ‖ P4-LLM-6 → P5-MODELS-3 ‖ its wave-3 queue | IA-1 refactors the settings form SET-4/5 built, so it follows SET-5 in the same lane; UX-1 and IA-2 are the web chain of wave 1 |
| `p5-platform` | P5-FILES gate ‖ FIX-156 → FIX-155 ‖ **P5-UX-4** (wave 1; jumps ahead as soon as P5-UX-2, P5-IA-1 and P5-UX-1 are merged) ‖ P4-SSH-1 → P4-SSH-2 ‖ wave-2/3 UX work placed as lanes free ‖ its wave-3 queue | UX-4 runs beside P5-IA-2 and P5-UX-13 (disjoint by brief); the lane is the only one free of the registry and the settings form |

**Rules for this work:**
- **Release gate (D-352):** after FIX-159, master is not deployed between the first other wave-0 merge (P5-UX-2 or P5-IA-1) and the end of wave 1 (P5-UX-1, P5-IA-2, P5-UX-4, P5-UX-13 merged). PROGRESS's header says when the window is open.
- **Registry:** FIX-159 → P5-UX-2 → P5-UX-13, then P5-GUIDE-1/3 and P5-ICS (still in `p5-apps`' wave-3 queue, so after the UX web chain anyway).
- **Settings form:** SET-5 → P5-IA-1; P5-IA-4 and P5-IA-5 after SET-4 (merged). SET-4/5 add no sidebar entries (D-340); P5-IA-1 owns all navigation.
- **Approvals:** P5-UX-16 and P4-011B-1..5 never run at the same time; whichever starts first finishes first.
- **P5-WEBHOOK-1..4:** 1–3 are placed when a lane frees; P5-WEBHOOK-4 builds inside the new structure (its connect steps are the journey's sign-in step, its card an account row), so after P5-UX-3 and P5-UX-4.
- **Migrations:** P5-IA-1 has `20261201000244`; P5-UX-5, P5-UX-11, P5-UX-14 and P5-UX-17 take the next free number when they start.
- **End check:** when wave 3 is done, the vocabulary test's exempt-file list (P5-IA-5, C-974) is empty.

## Coordinator (Opus) work, in order
1. **Spec step 0** for FIX-142, FIX-145, FIX-146 (unblocks wave 1/2 lanes).
2. **SPEC-SETTINGS** spec batch → SET-1..SET-5 briefs (owner answers its open classification questions).
3. **P5-CHAT-0** design boards + `docs/14` (owner reviews before P5-CHAT-2).
4. Specs/briefs for the rows with none, in the owner's order (D-235(c)): **P4-LLM, P4-SSH-1/2, P4-011B** first; then P4-S3, P4-EVAL, P4-NARR, P4-UTOK (P5-SLACK moved to phase 6, D-285; P4-S3, specified, moved to phase 6, D-318).

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
