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
  kind: "schedule" | "webhook";
  enabled: boolean;
  /** A one-time schedule that ran: it can be deleted, but is neither paused nor resumed. */
  done?: boolean;
}

/**
 * (P5-REFINE-19, `bulk`, Expert) The bar over the Automations list: **Pause**, **Resume** (the undoable rung: they act at once and
 * a toast offers **Undo**) and **Delete…** (the permanent rung: type the count). The selection is schedules and webhooks (a chat
 * row has no checkbox and no Pause). Each goes through the per-item route in turn ("2 of 3"); a failure is listed by name with Retry.
 */
export function AutomationsBulkBar({ agentId, picked, onClear }: { agentId: string; picked: BulkAutomation[]; onClear: () => void }) {
  const bulk = useBulk();
  const [deleting, setDeleting] = useState(false);
  const updSchedule = useUpdateSchedule(agentId);
  const updWebhook = useUpdateWebhook(agentId);
  const delSchedule = useDeleteSchedule(agentId);
  const delWebhook = useDeleteWebhook(agentId);

  const kindOf = new Map(picked.map((p) => [p.id, p.kind]));
  const noun = (list: BulkAutomation[]) => {
    const kinds = new Set(list.map((p) => p.kind));
    return kinds.size === 1 ? (kinds.has("schedule") ? "schedule" : "webhook") : "automation";
  };
  const plural = (list: BulkAutomation[]) => `${list.length} ${noun(list)}${list.length === 1 ? "" : "s"}`;
  const setEnabled = (id: string, enabled: boolean) =>
    kindOf.get(id) === "schedule" ? updSchedule.mutateAsync({ id, body: { enabled } }) : updWebhook.mutateAsync({ id, body: { enabled } });

  const on = picked.filter((p) => p.enabled && !p.done);
  const off = picked.filter((p) => !p.enabled && !p.done);
  const targets = (list: BulkAutomation[]): BulkTarget[] => list.map((p) => ({ id: p.id, name: p.name }));

  async function toggle(list: BulkAutomation[], enabled: boolean) {
    const done = await bulk.run(targets(list), (t) => setEnabled(t.id, enabled));
    const ok = list.filter((p) => !done.failed.some((f) => f.id === p.id));
    if (ok.length === 0) return;
    showUndoToast(`${enabled ? "Resumed" : "Paused"} ${plural(ok)}`, () => {
      void bulk.run(targets(ok), (t) => setEnabled(t.id, !enabled));
    });
  }

  const word = noun(picked);
  return (
    <>
      <BulkBar count={picked.length} noun={`${word}${picked.length === 1 ? "" : "s"} selected`} bulk={bulk} onClear={onClear}>
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
        title={`Delete ${plural(picked)}?`}
        consequence={`${plural(picked)} are removed: schedules stop starting runs, and senders that call a deleted webhook get an error. Conversations they already started are kept. This cannot be undone.`}
        noun={word}
        typeLabel={`Type the number of ${word}s to confirm`}
        name={String(picked.length)}
        confirmLabel={`Delete ${plural(picked)}`}
        run={() => bulk.run(targets(picked), (t) => (kindOf.get(t.id) === "schedule" ? delSchedule.mutateAsync(t.id) : delWebhook.mutateAsync(t.id)))}
        onDone={onClear}
      />
    </>
  );
}
