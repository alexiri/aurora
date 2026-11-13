"use client";

import { useState } from "react";

import { ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ConfirmDialog } from "@/components/ui/confirm-dialog";
import { DataTable, Td, Th } from "@/components/ui/data-table";
import { Notice } from "@/components/ui/notice";
import { Section } from "@/components/ui/section";
import { StatusDot } from "@/components/ui/status-dot";
import { errorMessage } from "@/lib/api/errors";
import { useRetrySandbox, useRestartSandbox, useSandbox, useStartFailures } from "@/lib/api/conversations";
import type { SandboxStartAttempt, StartLogEntry } from "@/lib/api/conversations";
import { useInstance, useOrg, useResolve } from "@/lib/api/queries";
import { useParams } from "next/navigation";
import { ShareDialog } from "@/components/agents/previews-tab";
import { clockTime } from "@/lib/chat/layout";
import { errorCopy } from "@/lib/chat/transcript";

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

export function ComputerTab({ agentId, frozen = false }: { agentId: string; frozen?: boolean }) {
  const q = useSandbox(agentId);
  const restart = useRestartSandbox(agentId);
  const retry = useRetrySandbox(agentId);
  const [open, setOpen] = useState(false);
  // (P4-PRV-1) Share… opens the add-share dialog prefilled with a listening port. The action exists
  // only when the org has switched Port previews on (docs/14): the instance flag alone is not enough.
  const instance = useInstance();
  const { org } = useParams<{ org: string }>();
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
  const tone = v.state === "running" ? "accent" : v.state === "failed" ? "danger" : v.state === "starting" || v.state === "stopping" ? "pending" : "neutral";
  return (
    <div className="space-y-8">
      <Section
        id="computer-state"
        title="Computer"
        description={v.image ? `Image: ${v.image}` : undefined}
        actions={
          <Button variant="outline" size="sm" disabled={restart.isPending || restartBlocked} onClick={() => setOpen(true)}>
            Restart
          </Button>
        }
        card
      >
        <div className="flex flex-wrap items-center gap-2 text-sm">
          State: <StatusDot tone={tone}>{v.state}</StatusDot>
        </div>
      </Section>
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
      {restart.error ? (
        <Notice tone="danger" role="alert">
          {errorMessage(restart.error)}
        </Notice>
      ) : null}
      {v.restart_to_update ? (
        <Notice role="status">Configuration changed. Restart to update this computer.</Notice>
      ) : null}
      {v.state === "failed" ? (
        <Notice tone="danger" role="alert">
          <div className="flex flex-wrap items-center gap-x-3 gap-y-1.5">
            <span className="min-w-0 flex-1 basis-full sm:basis-auto">
              {errorCopy(v.failure_code).message} {v.failure_message ?? ""}
            </span>
            {/* (FIX-029, C-615) Retry now: disabled "Start in flight…" once the start leaves `failed` (the 202 view parks it in `stopped`). */}
            {/* (FIX-049, C-615) Hidden on a frozen agent: the API would answer 423. */}
            {!frozen && (v.retryable || retry.isPending) ? (
              <Button
                size="sm"
                variant="outline"
                disabled={retry.isPending || !v.retryable}
                onClick={() => retry.mutate(undefined)}
              >
                {retry.isPending || !v.retryable ? "Start in flight…" : "Retry now"}
              </Button>
            ) : null}
          </div>
          {retry.error ? (
            <p role="alert" className="mt-1 text-xs">
              {errorMessage(retry.error)}
            </p>
          ) : null}
        </Notice>
      ) : null}
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
  );
}
