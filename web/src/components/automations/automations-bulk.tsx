"use client";

import { useState } from "react";

import { BulkBar } from "@/components/ui/bulk-bar";
import { Button } from "@/components/ui/button";
import { TypeToConfirm } from "@/components/ui/type-to-confirm";
import { showUndoToast } from "@/components/ui/undo-toast";
import { useDeleteSchedule, useUpdateSchedule } from "@/lib/api/schedules";
import { useDeleteWebhook, useUpdateWebhook } from "@/lib/api/webhooks";
import { useBulk, type BulkTarget } from "@/lib/bulk";

export interface BulkAutomation {
  id: string;
  name: string;
  enabled: boolean;
}

/**
 * (P5-REFINE-19, `bulk`, Expert) The bar over a list of an agent's schedules or webhooks: **Pause**, **Resume** (the undoable
 * rung: they act at once and a toast offers **Undo**) and **Delete…** (the permanent rung: type the count). Each goes through
 * the per-item route in turn ("2 of 3"); a failure is listed by name with Retry.
 */
export function AutomationsBulkBar({ kind, agentId, picked, onClear }: { kind: "schedule" | "webhook"; agentId: string; picked: BulkAutomation[]; onClear: () => void }) {
  const bulk = useBulk();
  const [deleting, setDeleting] = useState(false);
  const updSchedule = useUpdateSchedule(agentId);
  const updWebhook = useUpdateWebhook(agentId);
  const delSchedule = useDeleteSchedule(agentId);
  const delWebhook = useDeleteWebhook(agentId);

  const noun = kind === "schedule" ? "schedule" : "webhook";
  const plural = (n: number) => `${n} ${noun}${n === 1 ? "" : "s"}`;
  const setEnabled = (id: string, enabled: boolean) =>
    kind === "schedule" ? updSchedule.mutateAsync({ id, body: { enabled } }) : updWebhook.mutateAsync({ id, body: { enabled } });

  const on = picked.filter((p) => p.enabled);
  const off = picked.filter((p) => !p.enabled);
  const targets = (list: BulkAutomation[]): BulkTarget[] => list.map((p) => ({ id: p.id, name: p.name }));

  async function toggle(list: BulkAutomation[], enabled: boolean) {
    const done = await bulk.run(targets(list), (t) => setEnabled(t.id, enabled));
    const okIds = list.filter((p) => !done.failed.some((f) => f.id === p.id));
    if (okIds.length === 0) return;
    showUndoToast(`${enabled ? "Resumed" : "Paused"} ${plural(okIds.length)}`, () => {
      void bulk.run(targets(okIds), (t) => setEnabled(t.id, !enabled));
    });
  }

  return (
    <>
      <BulkBar count={picked.length} noun={`${noun}${picked.length === 1 ? "" : "s"} selected`} bulk={bulk} onClear={onClear}>
        {on.length > 0 ? (
          <Button type="button" size="sm" variant="outline" disabled={bulk.running} onClick={() => void toggle(on, false)}>
            Pause
          </Button>
        ) : null}
        {off.length > 0 ? (
          <Button type="button" size="sm" variant="outline" disabled={bulk.running} onClick={() => void toggle(off, true)}>
            Resume
          </Button>
        ) : null}
        <Button type="button" size="sm" variant="outline" disabled={bulk.running || picked.length === 0} onClick={() => setDeleting(true)}>
          Delete…
        </Button>
      </BulkBar>
      <TypeToConfirm
        open={deleting}
        onOpenChange={setDeleting}
        title={`Delete ${plural(picked.length)}?`}
        consequence={
          kind === "schedule"
            ? `${plural(picked.length)} stop starting runs now. Conversations they already started are kept. This cannot be undone.`
            : `${plural(picked.length)} and their delivery history are removed, and senders that call them get an error. Conversations they started are kept. This cannot be undone.`
        }
        noun={noun}
        typeLabel={`Type the number of ${noun}s to confirm`}
        name={String(picked.length)}
        confirmLabel={`Delete ${plural(picked.length)}`}
        run={() =>
          bulk.run(targets(picked), (t) => (kind === "schedule" ? delSchedule.mutateAsync(t.id) : delWebhook.mutateAsync(t.id)))
        }
        onDone={onClear}
      />
    </>
  );
}
