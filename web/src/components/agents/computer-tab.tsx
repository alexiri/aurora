"use client";

import { useState } from "react";

import { ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import { Dialog, DialogClose, DialogContent, DialogTrigger } from "@/components/ui/dialog";
import { errorMessage } from "@/lib/api/errors";
import { useRestartSandbox, useSandbox } from "@/lib/api/conversations";
import { errorCopy } from "@/lib/chat/transcript";

const REASONS: Record<string, string> = {
  active_turn: "A turn is running",
  busy_process: "A process is busy",
  schedule_soon: "A schedule fires soon",
  recent_activity: "Recent activity",
  open_session: "An interactive session is open",
};
export const reasonLabel = (r: string) => REASONS[r] ?? r.replace(/_/g, " ");

export function ComputerTab({ agentId }: { agentId: string }) {
  const q = useSandbox(agentId);
  const restart = useRestartSandbox(agentId);
  const [open, setOpen] = useState(false);
  if (q.isPending) return <Loading />;
  if (q.isError) return <ErrorState error={q.error} onRetry={() => void q.refetch()} />;
  const v = q.data;
  return (
    <div className="space-y-4">
      <div className="flex items-center gap-3">
        <p>
          State: <span className="font-medium">{v.state}</span>
        </p>
        <Dialog open={open} onOpenChange={setOpen}>
          <DialogTrigger asChild>
            <Button variant="outline" size="sm" disabled={restart.isPending}>
              Restart
            </Button>
          </DialogTrigger>
          <DialogContent title="Restart this computer?" description="Running processes will be stopped and the computer boots again.">
            <div className="flex justify-end gap-2">
              <DialogClose asChild>
                <Button variant="outline">Cancel</Button>
              </DialogClose>
              <Button
                onClick={() => {
                  restart.mutate(undefined, { onSettled: () => setOpen(false) });
                }}
              >
                Restart
              </Button>
            </div>
          </DialogContent>
        </Dialog>
      </div>
      {restart.error ? (
        <p role="alert" className="text-sm text-red-700">
          {errorMessage(restart.error)}
        </p>
      ) : null}
      {v.restart_to_update ? (
        <p role="status" className="rounded bg-blue-50 p-2 text-sm text-blue-900">
          Configuration changed. Restart to update this computer.
        </p>
      ) : null}
      {v.state === "failed" ? (
        <p role="alert" className="text-sm text-red-700">
          {errorCopy(v.failure_code).message} {v.failure_message ?? ""}
        </p>
      ) : null}
      {v.image ? <p className="text-sm text-neutral-600">Image: {v.image}</p> : null}
      <section aria-label="Awake reasons">
        <h2 className="text-sm font-semibold">Awake because</h2>
        {v.keep_awake_reasons.length ? (
          <ul className="list-disc pl-5 text-sm">
            {v.keep_awake_reasons.map((r) => (
              <li key={r}>{reasonLabel(r)}</li>
            ))}
          </ul>
        ) : (
          <p className="text-sm text-neutral-600">Nothing is keeping this computer awake.</p>
        )}
      </section>
      <section aria-label="Processes">
        <h2 className="text-sm font-semibold">Processes</h2>
        {v.processes.length ? (
          <table className="w-full text-left text-sm">
            <thead>
              <tr>
                <th scope="col">Name</th>
                <th scope="col">State</th>
                <th scope="col">Busy</th>
                <th scope="col">Started</th>
              </tr>
            </thead>
            <tbody>
              {v.processes.map((p) => (
                <tr key={p.id}>
                  <td>{p.name}</td>
                  <td>{p.state}</td>
                  <td>{p.busy ? "yes" : "no"}</td>
                  <td>{p.started_at ?? ""}</td>
                </tr>
              ))}
            </tbody>
          </table>
        ) : (
          <p className="text-sm text-neutral-600">No processes.</p>
        )}
      </section>
    </div>
  );
}
