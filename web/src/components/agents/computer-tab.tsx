"use client";

import { useState } from "react";

import { ErrorState, Loading } from "@/components/states";
import { Altitude } from "@/components/ui/altitude";
import { Badge } from "@/components/ui/badge";
import { FromYourTerminal } from "@/components/ui/from-your-terminal";
import { Button } from "@/components/ui/button";
import { DataTable, Td, Th } from "@/components/ui/data-table";
import { Section } from "@/components/ui/section";
import { StatusLine, type StatusTone } from "@/components/ui/status-line";
import { Segmented } from "@/components/ui/segmented";
import { askSentence } from "@/lib/who-can";
import { useSandbox, useStartFailures } from "@/lib/api/conversations";
import type { SandboxStartAttempt, StartLogEntry } from "@/lib/api/conversations";
import { useInstance, useMe, useOrg, useResolve } from "@/lib/api/queries";
import { useParams } from "next/navigation";
import { UsageCard } from "@/components/usage/usage-card";
import { ShareDialog } from "@/components/agents/previews-tab";
import { clockTime } from "@/lib/chat/layout";
import { COMPUTER_WORD, label } from "@/lib/labels";
import { TechnicalDetails } from "@/components/ui/technical-details";

import { computerOf } from "./computer-state";
import { readShell, SHELL_OPTIONS, writeShell, type Shell } from "./terminal-shell";

/**
 * (FIX-138, D-246) The terminal's shell, on the Computer page rather than above the pane (owner 2026-11-14). Still
 * remembered per browser and agent, so it applies at once and to the next terminal session only.
 */
function TerminalShellSection({ agentId }: { agentId: string }) {
  // Lazy initial read: the agent page renders its pages client-side only (inside its agent query), so no hydration mismatch.
  const [shell, setShell] = useState<Shell>(() => readShell(agentId));
  const [unsaved, setUnsaved] = useState(false);
  return (
    <Section
      id="terminal-shell"
      title="Terminal"
      description="The shell a new terminal session starts. Remembered in this browser; an open terminal keeps its shell until you reconnect."
      card
    >
      <Segmented<Shell>
        label="Shell"
        value={shell}
        options={SHELL_OPTIONS}
        onChange={(s) => {
          setShell(s);
          setUnsaved(!writeShell(agentId, s));
        }}
      />
      {unsaved ? (
        <p role="status" className="mt-2 text-xs text-muted">
          This browser blocks storage, so the choice was not remembered; terminals start bash.
        </p>
      ) : null}
    </Section>
  );
}

const REASONS: Record<string, string> = {
  active_turn: "A turn is running",
  busy_process: "A process is busy",
  schedule_soon: "A schedule fires soon",
  recent_activity: "Recent activity",
  open_session: "An interactive session is open",
  // (P4-PRV-1, D-145) the preview keep-awake reason.
  preview: "Preview in use",
  // (P4-TERM, C-621) the terminal keep-awake reason.
  terminal: "Terminal in use",
};
export const reasonLabel = (r: string) => REASONS[r] ?? r.replace(/_/g, " ");

/** (FIX-025, C-584) The last start attempt: phase, elapsed, reason detail and the pod's events. */
function StartAttempt({ a }: { a: SandboxStartAttempt }) {
  const started = new Date(a.started_at);
  const elapsed = a.progress_at
    ? Math.max(0, Math.round((new Date(a.progress_at).getTime() - started.getTime()) / 1000))
    : null;
  return (
    <Section id="start-attempt" title="Last start attempt" card>
      <dl className="grid grid-cols-[auto_1fr] items-baseline gap-x-4 gap-y-1 text-sm">
        <dt className="text-muted">Started</dt>
        <dd>{Number.isNaN(started.getTime()) ? "" : clockTime(a.started_at)}</dd>
        {a.phase ? (
          <>
            <dt className="text-muted">Phase</dt>
            <dd className="break-words">
              {a.phase}
              {a.detail ? <span className="text-ink-2">: {a.detail}</span> : null}
            </dd>
          </>
        ) : null}
        {elapsed !== null && !Number.isNaN(started.getTime()) ? (
          <>
            <dt className="text-muted">Last report</dt>
            <dd>{elapsed >= 90 ? `${Math.floor(elapsed / 60)} min ${elapsed % 60} s` : `${elapsed} s`} after start</dd>
          </>
        ) : null}
      </dl>
      {a.events.length ? (
        <div className="mt-3">
          <p className="mb-1 text-xs text-muted">Cluster events</p>
          <ul className="space-y-1">
            {a.events.map((e, i) => (
              <li key={i} className="break-words font-mono text-xs text-ink-2">
                {e}
              </li>
            ))}
          </ul>
        </div>
      ) : null}
    </Section>
  );
}

/** (FIX-029, C-612) One entry of the boot's start log: the phase (or the retry note) and its detail. */
function StartLogRow({ e }: { e: StartLogEntry }) {
  const label = e.kind === "note" ? (e.phase === "retry" ? "Retry" : "Note") : (e.phase ?? "phase");
  return (
    <li className="break-words text-sm">
      <span className="whitespace-nowrap font-mono text-xs text-muted">{clockTime(e.ts)}</span>{" "}
      <span className="font-medium">{label}</span>
      {e.detail ? <span className="text-ink-2">: {e.detail}</span> : null}
    </li>
  );
}

/** (FIX-029, C-615) The **Start log** list under **Last start attempt**, with the failure-history link. */
function StartLog({ agentId, log }: { agentId: string; log: StartLogEntry[] }) {
  const failures = useStartFailures(agentId);
  return (
    <Section id="start-log" title="Start log" description="Phases and notes of the current or last start attempt, newest first." card>
      {log.length ? (
        <ul className="space-y-1">
          {log.map((e, i) => (
            <StartLogRow key={`${e.ts}-${i}`} e={e} />
          ))}
        </ul>
      ) : (
        <p className="text-sm text-muted">Nothing logged for this attempt yet.</p>
      )}
      <p className="mt-3 text-sm">
        <a className="font-semibold underline" href="#start-failure-history">
          Recent start failures
        </a>
      </p>
      <div id="start-failure-history" className="mt-2">
        {failures.isPending ? null : failures.isError ? null : failures.data.items.length ? (
          <ul className="space-y-1">
            {failures.data.items.map((f) => (
              <li key={f.id} className="break-words text-sm text-ink-2">
                <span className="whitespace-nowrap font-mono text-xs text-muted">{clockTime(f.created_at)}</span>{" "}
                <span className="font-medium">{f.code}</span>: {f.message}
              </li>
            ))}
          </ul>
        ) : (
          <p className="text-sm text-muted">No start failures in the last 30 days.</p>
        )}
      </div>
    </Section>
  );
}

type Word = ReturnType<typeof computerOf>["word"];

const FACT_TONE: Record<Word, StatusTone> = { on: "ok", off: "idle", starting: "idle", stopping: "idle", failed: "fail" };

/** (D-412, computer-7) What a member reads in Details: "Started 23:55 · running" while it runs, else the phase in words. */
function factLine(word: Word, startedAt: string | null | undefined): string {
  if (word === "on") return startedAt ? `Started ${clockTime(startedAt)} · running` : "Running";
  if (word === "starting") return "Starting…";
  if (word === "stopping") return "Stopping…";
  if (word === "failed") return "Couldn't start";
  return label(COMPUTER_WORD, word);
}

/** Raw phases the word already says: no secondary line repeats them ("On" over a raw "ready", computer-7). */
const PHASE_SAID: Record<Word, readonly string[]> = {
  on: ["ready", "running"],
  off: ["stopped", "unprovisioned", "off"],
  starting: ["starting"],
  stopping: ["stopping"],
  failed: ["failed"],
};

/** "A", "A and B", "A, B and C". */
function listWords(items: string[]): string {
  return items.length <= 1 ? items.join("") : `${items.slice(0, -1).join(", ")} and ${items[items.length - 1]}`;
}

/**
 * (P5-REFINE-15, D-408, D-412) **Details**, the Computer's last sub-page (Overview when the instance has no Files): the
 * usage card, ports, and what the computer is. The state line above every sub-page is `ComputerStateLine`. A member
 * reads "Started 23:55 · on" and nothing about the machinery (SEC-141: the API view does not carry the image or runner to
 * them); an instance admin also gets the **Admin only** fold; the raw phase is a secondary line at Expert.
 */
export function ComputerTab({
  agentId,
  agentName,
  agentHandle,
  terminal = false,
  workspaceId,
}: {
  agentId: string;
  /** For the sentences ("Daneel's computer"). */
  agentName?: string;
  /** For the commands of the "From your terminal" fold (Expert). */
  agentHandle?: string;
  /** The agent's workspace: the Usage card reads its usage (C-1000). */
  workspaceId?: string;
  frozen?: boolean;
  /** The instance's `terminal.enabled`: the Terminal card shows only where the Terminal page exists. */
  terminal?: boolean;
}) {
  const q = useSandbox(agentId);
  const me = useMe();
  // (P4-PRV-1) Share… opens the add-share dialog prefilled with a listening port. The action exists
  // only when the org has switched Port previews on (docs/14): the instance flag alone is not enough.
  const instance = useInstance();
  const { org, ws } = useParams<{ org: string; ws: string }>();
  const resolved = useResolve(org);
  const orgQ = useOrg(resolved.data?.org.id);
  const previewsUsable = instance.data?.features.previews === true && orgQ.data?.settings.previews_enabled === true;
  const [sharePort, setSharePort] = useState<number | null>(null);
  if (q.isPending) return <Loading />;
  if (q.isError) return <ErrorState error={q.error} onRetry={() => void q.refetch()} />;
  const v = q.data;
  const c = computerOf(v);
  const name = agentName ?? "The agent";
  const isAdmin = me.data?.user.is_instance_admin === true;
  const missing = [
    ...(terminal ? [] : ["Terminal", "Files"]),
    ...(previewsUsable ? [] : ["Previews"]),
    ...(instance.data?.features.ssh === true ? [] : ["SSH"]),
  ];
  return (
    <div className="space-y-6">
      {missing.length > 0 ? (
        <p className="text-sm text-ink-2" data-testid="missing-pages">
          {listWords(missing)} {missing.length === 1 ? "isn't" : "aren't"} set up on this Aurora. {askSentence({ role: "instance_admin" })}
        </p>
      ) : null}
      {/* (D-412) What a member reads: the phase in words and when it started; nothing else. */}
      <section aria-label={`${name}'s computer`} className="space-y-1 rounded-card border border-line bg-surface p-4 shadow-card sm:p-5" data-testid="computer-facts">
        <h2 className="font-display text-base font-semibold text-ink">{name}&apos;s computer</h2>
        {/* (P5-REFINE-20 #12, computer-7) "Started 23:55 · running" only while it runs: an off computer is not "Started … · off". */}
        <StatusLine tone={FACT_TONE[c.word]} data-testid="computer-started">
          {factLine(c.word, c.started_at)}
        </StatusLine>
        <Altitude id="computer.phase" level="expert">
          {c.phase && !PHASE_SAID[c.word].includes(c.phase) ? (
            <p className="font-mono text-xs text-muted" data-testid="computer-phase">
              {c.phase}
            </p>
          ) : null}
        </Altitude>
      </section>
      {workspaceId ? <UsageCard wsId={workspaceId} org={org} ws={ws} agentId={agentId} /> : null}
      {terminal ? (
        <Altitude id="agent.terminal" level="expert">
          <TerminalShellSection agentId={agentId} />
        </Altitude>
      ) : null}
      {sharePort !== null ? <ShareDialog agentId={agentId} prefillPort={sharePort} onDone={() => setSharePort(null)} /> : null}
      {v.ports.length ? (
        <Section id="computer-ports" title="Ports" description="TCP ports currently listening inside this computer.">
          <DataTable caption="Listening ports">
            <thead>
              <tr>
                <Th>Port</Th>
                <Th>Protocol</Th>
                <Th>Shared</Th>
                <Th>
                  <span className="sr-only">Actions</span>
                </Th>
              </tr>
            </thead>
            <tbody>
              {v.ports.map((p) => (
                <tr key={p.port}>
                  <Td className="font-mono">{p.port}</Td>
                  <Td>{p.protocol}</Td>
                  <Td>{p.shared ? <Badge tone="accent">Shared</Badge> : <span className="text-muted">—</span>}</Td>
                  <Td className="text-right">
                    {previewsUsable ? (
                      <Button size="sm" variant="outline" onClick={() => setSharePort(p.port)}>
                        {p.shared ? "Shares" : "Share…"}
                      </Button>
                    ) : null}
                  </Td>
                </tr>
              ))}
            </tbody>
          </DataTable>
        </Section>
      ) : null}
      {/* (P5-REFINE-19, Expert) The two commands the CLI has for a computer: ssh into it, run a command with its proxy. */}
      {agentHandle ? (
        <FromYourTerminal
          commands={[
            ...(instance.data?.features.ssh === true
              ? [{ label: "the aurora ssh command", text: `aurora ssh ${agentHandle}`, note: "Open a shell on this computer: it fetches a short-lived certificate for your SSH key and starts ssh." }]
              : []),
            { label: "the aurora run command", text: `aurora run --agent ${agentHandle} -- <command>`, note: "Run a command on your own machine with this agent's proxy and certificate settings in its environment." },
          ]}
        />
      ) : null}
      {/* (D-412, SEC-141) The machinery is for instance admins only. The API does not send it to anyone else; this is not the
          only guard: nothing is read from a member's view. */}
      {isAdmin ? (
        <section aria-labelledby="admin-only-h" className="space-y-2">
          <h2 id="admin-only-h" className="text-sm font-semibold text-ink">
            Admin only
          </h2>
          <TechnicalDetails
            className="rounded-card border border-line bg-surface px-4 py-3 shadow-card"
            summaryClassName="font-medium text-ink"
            bodyClassName="mt-4"
            data-testid="technical-details"
          >
            <div className="space-y-8">
              <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 text-sm" data-testid="admin-facts">
                {v.image ? (
                  <>
                    <dt className="text-muted">Image</dt>
                    <dd className="break-all font-mono text-xs">{v.image}</dd>
                  </>
                ) : null}
                {v.runner_id ? (
                  <>
                    <dt className="text-muted">Runner</dt>
                    <dd className="break-all font-mono text-xs">{v.runner_id}</dd>
                  </>
                ) : null}
                <dt className="text-muted">State</dt>
                <dd className="font-mono text-xs">
                  {v.state}
                  {c.phase ? ` · ${c.phase}` : ""}
                </dd>
              </dl>
              {v.start_attempt ? <StartAttempt a={v.start_attempt} /> : null}
              {/* (FIX-029, C-612/C-615) The boot's start log, live while starting, frozen at the failure. */}
              <StartLog agentId={agentId} log={v.start_log} />
              <Section id="awake-reasons" title="Awake because" card>
                {v.keep_awake_reasons.length ? (
                  <ul className="flex flex-wrap gap-2">
                    {v.keep_awake_reasons.map((r) => (
                      <li key={r}>
                        <Badge tone="accent" className="normal-case tracking-normal">
                          {reasonLabel(r)}
                        </Badge>
                      </li>
                    ))}
                  </ul>
                ) : (
                  <p className="text-sm text-muted">Nothing is keeping this computer awake.</p>
                )}
              </Section>
              <Section id="processes" title="Processes">
                {v.processes.length ? (
                  <DataTable caption="Processes">
                    <thead>
                      <tr>
                        <Th>Name</Th>
                        <Th>State</Th>
                        <Th>Busy</Th>
                        <Th>Started</Th>
                      </tr>
                    </thead>
                    <tbody>
                      {v.processes.map((p) => (
                        <tr key={p.id}>
                          <Td className="font-mono">{p.name}</Td>
                          <Td>{p.state}</Td>
                          <Td>{p.busy ? "yes" : "no"}</Td>
                          <Td className="whitespace-nowrap text-ink-2">{p.started_at ?? ""}</Td>
                        </tr>
                      ))}
                    </tbody>
                  </DataTable>
                ) : (
                  <p className="text-sm text-muted">No processes.</p>
                )}
              </Section>
            </div>
          </TechnicalDetails>
        </section>
      ) : null}
    </div>
  );
}
