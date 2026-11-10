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
import { useRestartSandbox, useSandbox } from "@/lib/api/conversations";
import type { SandboxStartAttempt } from "@/lib/api/conversations";
import { clockTime } from "@/lib/chat/layout";
import { errorCopy } from "@/lib/chat/transcript";

const REASONS: Record<string, string> = {
  active_turn: "A turn is running",
  busy_process: "A process is busy",
  schedule_soon: "A schedule fires soon",
  recent_activity: "Recent activity",
  open_session: "An interactive session is open",
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

export function ComputerTab({ agentId }: { agentId: string }) {
  const q = useSandbox(agentId);
  const restart = useRestartSandbox(agentId);
  const [open, setOpen] = useState(false);
  if (q.isPending) return <Loading />;
  if (q.isError) return <ErrorState error={q.error} onRetry={() => void q.refetch()} />;
  const v = q.data;
  const tone = v.state === "running" ? "accent" : v.state === "failed" ? "danger" : v.state === "starting" || v.state === "stopping" ? "pending" : "neutral";
  return (
    <div className="space-y-8">
      <Section
        id="computer-state"
        title="Computer"
        description={v.image ? `Image: ${v.image}` : undefined}
        actions={
          <Button variant="outline" size="sm" disabled={restart.isPending} onClick={() => setOpen(true)}>
            Restart
          </Button>
        }
        card
      >
        <div className="flex flex-wrap items-center gap-2 text-sm">
          State: <StatusDot tone={tone}>{v.state}</StatusDot>
        </div>
      </Section>
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
          {errorCopy(v.failure_code).message} {v.failure_message ?? ""}
        </Notice>
      ) : null}
      {v.start_attempt ? <StartAttempt a={v.start_attempt} /> : null}
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
