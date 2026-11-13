"use client";

import { useState } from "react";

import { ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { DataTable, Td, Th } from "@/components/ui/data-table";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import { Notice } from "@/components/ui/notice";
import { PageHeader } from "@/components/ui/page-header";
import { api, ApiError } from "@/lib/api/client";
import { errorMessage } from "@/lib/api/errors";
import { useQuery } from "@tanstack/react-query";
import { clockTime } from "@/lib/chat/layout";

// FIX-025 (C-586): instance admins see each runner and its recent start failures, so a start
// that failed (`sandbox_start_failed`, `supervisor_unreachable`, …) is explained without kubectl.
// FIX-029 (C-614/C-615): a failure row names the agent; clicking it opens the **Start
// diagnostics** drawer with that agent's full failure history (the C-614 route).

/** The generated schema types; imported lazily via `unwrap`-style handling below. */
type RunnerList = { items: Array<Record<string, unknown>> };
type StartFailures = {
  items: {
    id: string;
    sandbox_id: string;
    agent_id: string;
    /** (C-614) The admin route names the runner; absent for a legacy row whose runner was deleted. */
    runner_id?: string | null;
    code: string;
    message: string;
    phase: string | null;
    events: string[];
    elapsed_secs: number | null;
    created_at: string;
  }[];
};

async function get<T>(path: string): Promise<T> {
  const { data, error, response } = await api.GET(path as never);
  if (error !== undefined) throw ApiError.fromBody(error, (response as Response).status);
  return data as T;
}

const UNWRAP = (v: unknown) => (typeof v === "string" ? v : "");

export function AdminRunnersView() {
  const runners = useQuery({ queryKey: ["admin", "runners"], queryFn: () => get<RunnerList>("/v1/admin/runners") });
  const [open, setOpen] = useState<string | null>(null);
  if (runners.isPending) return <Loading />;
  if (runners.isError) return <ErrorState error={runners.error} onRetry={() => void runners.refetch()} />;
  const items = runners.data.items;
  return (
    <div className="max-w-4xl space-y-6">
      <PageHeader
        title="Runners"
        description="Runners host the agents' computers. A start failure lists why the last start attempts failed (kept 30 days)."
      />
      {items.length === 0 ? (
        <p className="text-sm text-ink-2">No runners. Create one with POST /v1/admin/runners (docs/operator/runners.md).</p>
      ) : (
        <ul className="space-y-3">
          {items.map((r) => {
            const id = UNWRAP(r.id);
            const name = UNWRAP(r.name);
            const live = Number(r.live_replicas ?? 0);
            const backend = r.backend ? UNWRAP(r.backend) : null;
            return (
              <li key={id}>
                <Card>
                  <div className="mb-2 flex flex-wrap items-center justify-between gap-2">
                    <h2 className="text-base font-medium">{name}</h2>
                    <div className="flex items-center gap-2">
                      {backend ? <Badge>{backend}</Badge> : null}
                      <Badge tone={live > 0 ? "accent" : "danger"}>{live > 0 ? `${live} live` : "offline"}</Badge>
                    </div>
                  </div>
                  <Button variant="outline" size="sm" onClick={() => setOpen(open === id ? null : id)}>
                    {open === id ? "Hide start failures" : "Start failures"}
                  </Button>
                  {open === id ? <StartFailures runnerId={id} /> : null}
                </Card>
              </li>
            );
          })}
        </ul>
      )}
    </div>
  );
}

/** (FIX-029, C-614) The **Start diagnostics** drawer: an agent's start-failure history, newest first. */
function StartDiagnostics({ agentId, open, onOpenChange }: { agentId: string; open: boolean; onOpenChange: (v: boolean) => void }) {
  const q = useQuery({
    queryKey: ["admin", "agents", agentId, "start-failures"],
    queryFn: () => get<StartFailures>(`/v1/admin/agents/${agentId}/start-failures`),
    // Only for the drawer that is open on a real agent: never `/agents//start-failures`.
    enabled: open && agentId !== "",
    retry: false,
  });
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent title="Start diagnostics" description={`Start failures of agent ${agentId} (newest first, kept 30 days).`} className="w-[min(42rem,calc(100vw-2rem))]">
        {q.isPending ? (
          <Loading />
        ) : q.isError ? (
          <Notice tone="danger" role="alert">
            {errorMessage(q.error)}
          </Notice>
        ) : q.data.items.length === 0 ? (
          <p className="text-sm text-muted">No start failures in the last 30 days.</p>
        ) : (
          <DataTable caption="Start failures">
            <thead>
              <tr>
                <Th>When</Th>
                <Th>Code</Th>
                <Th>Runner</Th>
                <Th>Reason</Th>
                <Th>Phase</Th>
                <Th>Elapsed</Th>
              </tr>
            </thead>
            <tbody>
              {q.data.items.map((f) => (
                <tr key={f.id}>
                  <Td className="whitespace-nowrap">{clockTime(f.created_at)}</Td>
                  <Td>
                    <Badge tone="danger" className="normal-case tracking-normal">
                      {f.code}
                    </Badge>
                  </Td>
                  <Td className="max-w-[10rem] truncate font-mono text-xs">{f.runner_id ?? ""}</Td>
                  <Td className="max-w-[24rem] break-words">{f.message}</Td>
                  <Td>{f.phase ?? ""}</Td>
                  <Td className="whitespace-nowrap">{f.elapsed_secs !== null ? `${f.elapsed_secs} s` : ""}</Td>
                </tr>
              ))}
            </tbody>
          </DataTable>
        )}
      </DialogContent>
    </Dialog>
  );
}

function StartFailures({ runnerId }: { runnerId: string }) {
  const q = useQuery({
    queryKey: ["admin", "runners", runnerId, "start-failures"],
    queryFn: () => get<StartFailures>(`/v1/admin/runners/${runnerId}/start-failures`),
    retry: false,
  });
  const [agent, setAgent] = useState<string | null>(null);
  if (q.isPending) return <Loading />;
  if (q.isError) {
    return (
      <Notice tone="danger" role="alert" className="mt-3">
        {errorMessage(q.error)}
      </Notice>
    );
  }
  if (q.data.items.length === 0) {
    return <p className="mt-3 text-sm text-muted">No start failures in the last 30 days.</p>;
  }
  return (
    <div className="mt-3">
      <DataTable caption="Recent start failures">
        <thead>
          <tr>
            <Th>When</Th>
            <Th>Agent</Th>
            <Th>Code</Th>
            <Th>Reason</Th>
            <Th>Phase</Th>
            <Th>Elapsed</Th>
          </tr>
        </thead>
        <tbody>
          {q.data.items.map((f) => (
            // (FIX-029, C-615) The row names the agent and opens its Start diagnostics drawer.
            <tr
              key={f.id}
              className="cursor-pointer hover:bg-subtle"
              onClick={() => setAgent(f.agent_id)}
            >
              <Td className="whitespace-nowrap">{clockTime(f.created_at)}</Td>
              <Td>
                <Button variant="ghost" size="sm" className="h-auto p-0 font-mono text-xs" onClick={(e) => { e.stopPropagation(); setAgent(f.agent_id); }}>
                  {f.agent_id}
                </Button>
              </Td>
              <Td>
                <Badge tone="danger" className="normal-case tracking-normal">
                  {f.code}
                </Badge>
              </Td>
              <Td className="max-w-[28rem] break-words">{f.message}</Td>
              <Td>{f.phase ?? ""}</Td>
              <Td className="whitespace-nowrap">{f.elapsed_secs !== null ? `${f.elapsed_secs} s` : ""}</Td>
            </tr>
          ))}
        </tbody>
      </DataTable>
      {q.data.items.some((f) => f.events.length > 0) ? (
        <details className="mt-2 text-sm">
          <summary className="cursor-pointer text-muted">Cluster events</summary>
          <ul className="mt-1 space-y-1">
            {q.data.items.flatMap((f) =>
              f.events.map((e, i) => (
                <li key={`${f.id}-${i}`} className="break-words font-mono text-xs text-ink-2">
                  {e}
                </li>
              )),
            )}
          </ul>
        </details>
      ) : null}
      <StartDiagnostics agentId={agent ?? ""} open={agent !== null} onOpenChange={(v) => !v && setAgent(null)} />
    </div>
  );
}
