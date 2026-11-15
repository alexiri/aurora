"use client";

import { useState } from "react";

import { OneTimeKeyDialog } from "@/components/account/one-time-key-dialog";
import { ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { ConfirmDialog } from "@/components/ui/confirm-dialog";
import { DataTable, Td, Th } from "@/components/ui/data-table";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import { Field, inputClass } from "@/components/ui/field";
import { Notice } from "@/components/ui/notice";
import { PageHeader } from "@/components/ui/page-header";
import { api, ApiError } from "@/lib/api/client";
import { errorMessage } from "@/lib/api/errors";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
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

type Replica = { version?: string; last_seen_at?: string; live?: boolean };

/** Rotation grace choices (seconds); the server default is 10 minutes (C-695). */
const GRACES: { secs: number; label: string }[] = [
  { secs: 0, label: "No grace: the old token stops working now" },
  { secs: 600, label: "10 minutes (default)" },
  { secs: 3600, label: "1 hour" },
  { secs: 86400, label: "24 hours" },
];

async function rotateToken(id: string, graceSecs: number) {
  const { data, error, response } = await api.POST(
    `/v1/admin/runners/${id}/token?grace_secs=${graceSecs}` as never,
    {} as never,
  );
  if (error !== undefined) throw ApiError.fromBody(error, (response as Response).status);
  return data as { token: string; runner: Record<string, unknown> };
}

/** Newest `last_seen_at` of the runner's replicas, the versions of its live replicas. */
function replicaFacts(r: Record<string, unknown>) {
  const reps = (Array.isArray(r.replicas) ? r.replicas : []) as Replica[];
  const seen = reps.map((p) => p.last_seen_at ?? "").filter(Boolean).sort().at(-1);
  const versions = [...new Set(reps.filter((p) => p.live).map((p) => p.version ?? "").filter(Boolean))];
  return { seen, versions };
}

export function AdminRunnersView() {
  const qc = useQueryClient();
  const runners = useQuery({ queryKey: ["admin", "runners"], queryFn: () => get<RunnerList>("/v1/admin/runners") });
  const [open, setOpen] = useState<string | null>(null);
  const [rotating, setRotating] = useState<{ id: string; name: string } | null>(null);
  const [grace, setGrace] = useState(600);
  const [issued, setIssued] = useState<{ name: string; token: string; until: string | null; grace: number } | null>(null);
  const rotate = useMutation({
    gcTime: 0,
    mutationFn: (v: { id: string; graceSecs: number }) => rotateToken(v.id, v.graceSecs),
    onSuccess: (out, v) => {
      const until = out.runner.previous_token_expires_at;
      setIssued({
        name: rotating?.name ?? "",
        token: out.token,
        until: typeof until === "string" ? until : null,
        grace: v.graceSecs,
      });
      setRotating(null);
      void qc.invalidateQueries({ queryKey: ["admin", "runners"] });
    },
  });
  if (runners.isPending) return <Loading />;
  if (runners.isError) return <ErrorState error={runners.error} onRetry={() => void runners.refetch()} />;
  const items = runners.data.items;
  return (
    <div className="max-w-4xl space-y-6">
      <PageHeader
        title="Runners"
        description="Runners host the agents' computers. Capacity shows what each runner is running; a start failure lists why the last start attempts failed (kept 30 days). Adding and removing runners is done with Helm or the CLI (docs/operator/runners.md)."
      />
      {items.length === 0 ? (
        <p className="text-sm text-ink-2">No runners. Provision one with Helm or the CLI (docs/operator/runners.md).</p>
      ) : (
        <>
          <DataTable caption="Runner capacity">
            <thead>
              <tr>
                <Th>Runner</Th>
                <Th>Backend</Th>
                <Th>Sandboxes</Th>
                <Th>Replicas</Th>
                <Th>Version</Th>
                <Th>Last seen</Th>
              </tr>
            </thead>
            <tbody>
              {items.map((r) => {
                const f = replicaFacts(r);
                const max = typeof r.max_sandboxes === "number" ? r.max_sandboxes : null;
                const running = Number(r.running_sandboxes ?? 0);
                return (
                  <tr key={UNWRAP(r.id)}>
                    <Td>{UNWRAP(r.name)}</Td>
                    <Td>{r.backend ? UNWRAP(r.backend) : "not registered"}</Td>
                    <Td className="whitespace-nowrap">{max !== null ? `${running} of ${max} running` : `${running} running (no limit declared)`}</Td>
                    <Td>{Number(r.live_replicas ?? 0)} live</Td>
                    <Td>{f.versions.join(", ")}</Td>
                    <Td className="whitespace-nowrap">{f.seen ? clockTime(f.seen) : "never"}</Td>
                  </tr>
                );
              })}
            </tbody>
          </DataTable>
          <ul className="space-y-3">
            {items.map((r) => {
              const id = UNWRAP(r.id);
              const name = UNWRAP(r.name);
              const live = Number(r.live_replicas ?? 0);
              const backend = r.backend ? UNWRAP(r.backend) : null;
              const until = typeof r.previous_token_expires_at === "string" ? r.previous_token_expires_at : null;
              return (
                <li key={id}>
                  <Card>
                    <div className="mb-2 flex flex-wrap items-center justify-between gap-2">
                      <h2 className="text-base font-medium">{name}</h2>
                      <div className="flex flex-wrap items-center gap-2">
                        {backend ? <Badge>{backend}</Badge> : null}
                        {until ? <Badge tone="pending">old token valid until {clockTime(until)}</Badge> : null}
                        <Badge tone={live > 0 ? "accent" : "danger"}>{live > 0 ? `${live} live` : "offline"}</Badge>
                      </div>
                    </div>
                    <div className="flex flex-wrap gap-2">
                      <Button variant="outline" size="sm" onClick={() => setOpen(open === id ? null : id)}>
                        {open === id ? "Hide start failures" : "Start failures"}
                      </Button>
                      <Button
                        variant="outline"
                        size="sm"
                        onClick={() => {
                          rotate.reset();
                          setGrace(600);
                          setRotating({ id, name });
                        }}
                      >
                        Rotate token
                      </Button>
                    </div>
                    {open === id ? <StartFailures runnerId={id} /> : null}
                  </Card>
                </li>
              );
            })}
          </ul>
        </>
      )}
      <ConfirmDialog
        open={rotating !== null}
        onOpenChange={(o) => !o && setRotating(null)}
        title={`Rotate the token of ${rotating?.name ?? ""}?`}
        description="A new token is issued and shown once. The runner must be rolled to the new token; the old token keeps working for the grace period below, then is refused."
        confirmLabel="Rotate token"
        busy={rotate.isPending}
        onConfirm={() => rotating && rotate.mutate({ id: rotating.id, graceSecs: grace })}
      >
        <Field id="runner-grace" label="Grace period for the old token">
          <select id="runner-grace" className={inputClass} value={grace} onChange={(e) => setGrace(Number(e.target.value))}>
            {GRACES.map((g) => (
              <option key={g.secs} value={g.secs}>
                {g.label}
              </option>
            ))}
          </select>
        </Field>
        {rotate.isError ? (
          <Notice tone="danger" role="alert" className="mt-3">
            {errorMessage(rotate.error)}
          </Notice>
        ) : null}
      </ConfirmDialog>
      <OneTimeKeyDialog
        secret={issued?.token ?? null}
        title="Copy the new runner token"
        description={`New token for "${issued?.name ?? ""}". You won't see it again: update the runner's token Secret and roll its Deployment now. ${
          issued?.grace ? `The old token keeps working until ${issued.until ? clockTime(issued.until) : "the grace ends"}.` : "The old token no longer works."
        }`}
        onClose={() => setIssued(null)}
      />
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
      <DialogContent title="Start diagnostics" description={`Start failures of agent ${agentId} (newest first, kept 30 days).`} size="lg">
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
                  <Td className="max-w-40 truncate font-mono text-xs">{f.runner_id ?? ""}</Td>
                  <Td className="max-w-sm break-words">{f.message}</Td>
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
              <Td className="max-w-md break-words">{f.message}</Td>
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
