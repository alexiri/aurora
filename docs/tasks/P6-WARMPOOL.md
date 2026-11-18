# P6-WARMPOOL: Warm starts — the sandbox is ready when the person sends — **proposal, spec batch (Opus) later — phase 6**

> **Status: proposal (2026-11-18).** No C-, SEC- or migration numbers are taken (the spec batch takes
> them from PROGRESS's next-free note).
>
> **Owner answers, 2026-11-18 (D-379):** (Q-P6-WARM-1) warm on the **first keystroke** in the composer;
> (Q-P6-WARM-2) **on by default**; (Q-P6-WARM-3) **not in phase 5**: Parts 1 and 2 both ship **at the
> beginning of phase 6**, its first spec batch; (Q-P6-WARM-4) **wait for the measurements**: Part 3 and a
> true generic pool are decided from Part 1's numbers. No question is open.

## Goal

Owner, 2026-11-18: sandboxes "already take a bit to deploy"; asked for a **pool of warm sandboxes**.
The aim is that a person who writes to a parked agent does not wait for its computer to start.

Companions already briefed: FIX-175 (the sandbox image pre-pulled on every sandbox node, so no start pays
an image pull) and FIX-174 (smaller e2e image). This proposal covers the rest of the start.

## Today

- A start goes `claimed → waiting_capacity → provisioning → scheduling → creating → booting → ready`
  (`03` `sandboxes.start_phase`, `11 §Start`), bounded by `hosted.sandbox_ready_timeout_secs` (300 s).
  `ready` means the supervisor connected, claimed its bootstrap token and applied the home sync.
- A sandbox parks after `IDLE_STOP` (30 min, `hosted.idle_stop_secs`) with no keep-awake reason
  (`11 §Keep-awake`). The next message starts a new boot.
- **Wake demands** exist for previews, terminals, SSH and Files: a column (`preview_wake_at`,
  `terminal_wake_at`) that the due-work start arm treats as due for 60 s, a per-agent hourly budget, never
  for a frozen agent; and a keep-awake reason with `*_active_until`, a grace and a per-boot cap.
- **Nothing measures start time.** The per-boot `start_log` ring has timestamped phases but is reset by
  the next boot; there is no metric. Nobody knows today which phase dominates.

## Why not a classic pool of pre-started sandboxes

A classic pool keeps generic sandboxes running and hands one to whoever needs it. In Aurora a sandbox
is created *as* one agent's computer, and Kubernetes and podman cannot change that afterwards:

1. **The home is mounted at creation.** `/workspace` is the agent's own PVC (`home-<sandbox_id>`,
   `ReadWriteOnce`) or podman volume. A running Pod or container cannot gain a volume, so a generic
   sandbox can never become agent X's computer with X's files.
2. **Identity is fixed at creation.** The boot document (secret env / Secret `boot-<boot_id>`) carries
   the bootstrap and gateway boot tokens, the CA, the harness and the capabilities; Pod labels carry
   agent, sandbox and boot. Delivering these later would change the bootstrap-claim and boot-credential
   design that SEC invariants rest on.
3. **Size and isolation are fixed at creation.** Requests = limits come from the agent's resource
   profile; the runtime class (gVisor/Kata) is set per Pod. A pool would need one pool per profile.
4. **Idle pool members occupy capacity.** They count against `runner.max_sandboxes` and the sandbox
   namespace quota while serving nobody.

A real generic pool needs homes delivered over the network after claim (the `snapshot` home durability
named in the runner capabilities but never built). That is a redesign of homes, boot delivery and
reconcile: not proposed now (Q-P6-WARM-4).

## Proposal

### Part 1 — Measure first (small; ships with Part 2 at the start of phase 6, D-379)
- When a boot reaches `ready`, the API records a histogram `aurora_sandbox_start_seconds{backend, phase}`
  per phase (from the boot's `start_log` timestamps) and the total, with `first_boot` = this sandbox's
  first start (new home). Also count failures per phase.
- Admin › Runners shows the median and 95th percentile start time over the last 24 h per runner
  (computed from a small retained table or from the metric; the spec batch picks).
- Decide Parts 3 and Q-P6-WARM-4 from these numbers, after FIX-175 has removed image pulls.

### Part 2 — Warm on intent (the main change)
Start the agent's sandbox when a person shows they are about to use it, so the start runs while they type.
- **Signal** (D-379): the first keystroke in the agent's composer, sent by the dashboard
  as `POST /v1/agents/{agent}/sandbox/warm`; repeated at most every 60 s while the person keeps typing or
  the composer has text. Authority `can_use`; always `204` (it says nothing the caller cannot already see).
- **API:** stamps a wake demand `warm_wake_at` and `warm_active_until = now + WARM_GRACE` (120 s). The
  start arm treats a demand younger than `WARM_WAKE_WINDOW` (60 s) as due work, exactly like a terminal
  wake, with these limits:
  - never for a frozen agent, a disabled hosted plane or a sandbox in failure backoff;
  - **never evicts** a held-awake sandbox and **never waits for capacity**: if the runner has no free slot,
    the warm-up is dropped (the message itself, when sent, waits as today);
  - budget `WARM_WAKES_PER_AGENT_HOUR` (6) and a per-person rate limit.
- **Keep-awake reason `warm`:** `warm_active_until > now()` and less than `WARM_AWAKE_CAP` (10 min) since
  the first warm stamp of this boot. When a turn starts, normal engagement takes over. If the person
  leaves without sending, the sandbox parks after the grace (it does not wait `IDLE_STOP`: a warm-up does
  not stamp `last_engaged_at`).
- **Dashboard:** no new control. The agent page's status rail already shows "Sandbox starting"
  (`status-rail/status-data.ts`) for a warm sandbox that is not ready yet.
- **Setting** `hosted.warm_on_intent` (instance, default on, D-379; an admin can turn it off).
- **Cost:** a boot for every person who starts typing and then leaves, at most `WARM_AWAKE_CAP` each and
  bounded per agent and person. Warm time is runtime like any boot (`runtime_intervals`) and counts for
  P6-LIMITS as interactive time. Under P6-LIMITS' pressure level `elevated`, warm-ups are the first thing
  turned off.

### Part 3 — Pre-created homes for new agents (only if Part 1 shows a need)
A brand-new agent's first start also waits for its PVC to be provisioned (FIX-049 allows 90 s of
provisioning warnings). A pool of pre-created PVCs would need the home reference stored per sandbox
(today `home_ref_for` derives it from the sandbox id), reconcile rules for unassigned pool members, and
a storage class with `Immediate` binding (with `WaitForFirstConsumer` the disk is created at first
scheduling anyway, so a pool gains nothing). Recommended: only if `provisioning` on `first_boot` starts
is a large share of the time.

### Rejected
- **Generic pre-started sandboxes** (above).
- **Keeping sandboxes awake longer** for everyone: already a setting (`hosted.idle_stop_secs`); it costs
  compute for every agent, not only the ones about to be used.
- **Pausing instead of parking** (podman `pause`, cgroup freezer): instant resume, but memory stays held
  and Kubernetes has no equivalent.

## Deliverables of the spec batch (the first batch of phase 6, D-379)
- `11 §Keep-awake` (reason `warm`), `11 §Start` (warm demand rules), `05` (the route, its rate limit),
  `03 §Planned changes` (`warm_wake_at`, `warm_active_until`, `warm_awake_since` on `sandboxes`, and any
  retained start-time table), `15` (constants, `hosted.warm_on_intent`, the metrics), `14` (composer
  behaviour), `18` (tests: never evicts, never holds capacity, frozen agent, budget, parks after grace,
  turn takes over), one SEC invariant for the limits above.
- Briefs: P6-WARM-1 (Part 1, Rust + small web), P6-WARM-2 (Part 2, Rust + web). Part 3 only on evidence.

## Questions for the owner (all answered 2026-11-18, D-379)
- **Q-P6-WARM-1** Which signal starts a warm-up: the first keystroke in the composer, or opening the
  agent's chat? *Recommended: the keystroke. Opening a chat to read history would boot sandboxes nobody
  uses.* **Answered: the first keystroke.**
- **Q-P6-WARM-2** Is warm-on-intent on by default? *Recommended: on, as an instance setting an admin can
  turn off.* **Answered: on by default.**
- **Q-P6-WARM-3** When? Part 1 (measurement) is small and useful on its own. *Recommended: Part 1 now in
  phase 5 as a FIX brief; Part 2 with phase 6.* **Answered: not in phase 5; all of it at the beginning of
  phase 6.**
- **Q-P6-WARM-4** Should a true generic pool (homes delivered over the network, the `snapshot`
  durability) be explored later? *Recommended: no, unless Part 1 shows `booting` or `provisioning`
  dominating after FIX-175 and Part 2.* **Answered: wait for the measurements** (Part 3 likewise).
