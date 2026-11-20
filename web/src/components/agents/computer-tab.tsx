"use client";

import { useEffect, useState } from "react";

import { ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ConfirmDialog } from "@/components/ui/confirm-dialog";
import { DataTable, Td, Th } from "@/components/ui/data-table";
import { ErrorNotice, Notice } from "@/components/ui/notice";
import { Section } from "@/components/ui/section";
import { Segmented } from "@/components/ui/segmented";
import { StatusLine } from "@/components/ui/status-line";
import { askSentence } from "@/lib/who-can";
import { useRetrySandbox, useRestartSandbox, useSandbox, useStartFailures, useWakeSandbox } from "@/lib/api/conversations";
import { durationWords } from "@/lib/settings-words";
import type { SandboxStartAttempt, StartLogEntry } from "@/lib/api/conversations";
import { useInstance, useOrg, useResolve } from "@/lib/api/queries";
import { useParams } from "next/navigation";
import { UsageCard } from "@/components/usage/usage-card";
import { ShareDialog } from "@/components/agents/previews-tab";
import { clockTime } from "@/lib/chat/layout";
import { errorCopy } from "@/lib/chat/transcript";

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

/** States where Restart has nothing to do (the API answers 409 `restart_not_applicable`, C-626). */
const RESTART_NOT_APPLICABLE = new Set(["stopping", "stopped", "unprovisioned"]);

/** "A", "A and B", "A, B and C". */
function listWords(items: string[]): string {
  return items.length <= 1 ? items.join("") : `${items.slice(0, -1).join(", ")} and ${items[items.length - 1]}`;
}

export function ComputerTab({
  agentId,
  agentName,
  frozen = false,
  terminal = false,
  workspaceId,
}: {
  agentId: string;
  /** For the sentences ("Daneel's computer is asleep"). */
  agentName?: string;
  /** The agent's workspace: the Usage card reads its usage (C-1000). */
  workspaceId?: string;
  frozen?: boolean;
  /** The instance's `terminal.enabled`: the Terminal card shows only where the Terminal page exists. */
  terminal?: boolean;
}) {
  const q = useSandbox(agentId);
  const restart = useRestartSandbox(agentId);
  const retry = useRetrySandbox(agentId);
  const wake = useWakeSandbox(agentId);
  // (P5-APIGAP-1) After a 202 the button stays pending until the status says starting or running; a start that
  // never shows (the runner is busy) frees the button again after a minute.
  useEffect(() => {
    if (!wake.isSuccess) return;
    const t = setTimeout(() => wake.reset(), 60_000);
    return () => clearTimeout(t);
  }, [wake.isSuccess, wake]);
  const [open, setOpen] = useState(false);
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
  // (FIX-049, C-626) Restart is disabled where it would answer 409 (nothing to restart), and on a
  // failed computer of a frozen agent (the recovery would start work: 423).
  const restartBlocked = RESTART_NOT_APPLICABLE.has(v.state) || (v.state === "failed" && frozen);
  const name = agentName ?? "The agent";
  const asleep = v.state === "stopped" || v.state === "unprovisioned" || v.state === "stopping";
  const wakePending = wake.isPending || (wake.isSuccess && asleep);
  // (C-1046) In words, nothing when idle stop is off.
  const sleepsAfter = v.idle_stop_after_secs ? `Sleeps after ${durationWords(v.idle_stop_after_secs)} of quiet.` : null;
  const missing = [
    ...(terminal ? [] : ["Terminal", "Files"]),
    ...(previewsUsable ? [] : ["Previews"]),
    ...(instance.data?.features.ssh === true ? [] : ["SSH"]),
  ];
  return (
    <div className="space-y-6">
      {/* (P5-POLISH-6, C5) One sentence for the state, with the action that fits it. */}
      <section aria-label="Computer" className="space-y-3 rounded-card border border-line bg-surface p-4 shadow-card sm:p-5">
        <StatusLine tone={v.state === "running" ? "ok" : v.state === "failed" ? "fail" : "idle"} className="text-base" data-testid="computer-state">
          {v.state === "running"
            ? `${name}'s computer is awake`
            : v.state === "starting"
              ? `${name}'s computer is starting`
              : v.state === "failed"
                ? `${name}'s computer couldn't start`
                : `${name}'s computer is asleep`}
        </StatusLine>
        <p className="text-sm text-ink-2">
          {v.state === "failed"
            ? `${errorCopy(v.failure_code).message} ${v.failure_message ?? ""}`.trim()
            : v.state === "stopped" || v.state === "unprovisioned"
              ? [`It wakes by itself when ${name} needs it.`, sleepsAfter, "Its files stay."].filter(Boolean).join(" ")
              : v.state === "running"
                ? `${sleepsAfter ?? "It sleeps by itself when there is no work."} Its files stay.`
                : null}
        </p>
        <div className="flex flex-wrap gap-2">
          {asleep && !frozen ? (
            <Button size="sm" disabled={wakePending} onClick={() => wake.mutate(undefined)}>
              {wakePending ? "Waking…" : "Wake it now"}
            </Button>
          ) : null}
          {v.state === "running" ? (
            <Button variant="outline" size="sm" disabled={restart.isPending || restartBlocked} onClick={() => setOpen(true)}>
              Restart
            </Button>
          ) : null}
          {v.state === "failed" && !frozen ? (
            <Button size="sm" disabled={retry.isPending || !v.retryable} onClick={() => retry.mutate(undefined)}>
              {retry.isPending || !v.retryable ? "Start in flight…" : "Try again"}
            </Button>
          ) : null}
        </div>
        {missing.length > 0 ? (
          <p className="text-sm text-ink-2" data-testid="missing-pages">
            {listWords(missing)} {missing.length === 1 ? "isn't" : "aren't"} set up on this Aurora. {askSentence({ role: "instance_admin" })}
          </p>
        ) : null}
      </section>
      {workspaceId ? <UsageCard wsId={workspaceId} org={org} ws={ws} agentId={agentId} /> : null}
      {terminal ? <TerminalShellSection agentId={agentId} /> : null}
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
      <ConfirmDialog
        open={open}
        onOpenChange={setOpen}
        title="Restart this computer?"
        description="Running processes will be stopped and the computer boots again."
        confirmLabel="Restart"
        onConfirm={() => {
          restart.mutate(undefined, { onSettled: () => setOpen(false) });
        }}
      />
      <ErrorNotice error={restart.error} />
      <ErrorNotice error={wake.error} />
      {v.restart_to_update ? (
        <Notice role="status">Configuration changed. Restart to update this computer.</Notice>
      ) : null}
      {v.state === "failed" && retry.error ? (
        <ErrorNotice error={retry.error} />
      ) : null}
      {/* (P5-POLISH-6) The engine's details are collapsed: the last start attempt, the start log, why it is awake, the processes. */}
      <details className="rounded-card border border-line bg-surface px-4 py-3 shadow-card" data-testid="technical-details">
        <summary className="cursor-pointer text-sm font-medium text-ink">Technical details</summary>
        <div className="mt-4 space-y-8">
      {v.image ? <p className="text-xs text-muted">Image: {v.image}</p> : null}
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
      </details>
    </div>
  );
}
