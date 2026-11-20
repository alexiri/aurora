"use client";

import { useState } from "react";

import { Button } from "@/components/ui/button";
import { useWebhookView } from "@/lib/api/webhook";
import { effectiveLimitSentence } from "@/lib/webhook/logic";

import { WebhookEditDialog } from "./webhook-edit-dialog";

/**
 * What an Outgoing webhook adds to its account row (C-913): the host it sends to (never the address), the limit in force and
 * where it comes from and **Edit** (the test is **Send a test** in the row's menu, P5-REFINE-13). For people who manage the account: the reads need that right.
 */
export function WebhookCardLine({ connectionId, name }: { connectionId: string; name: string }) {
  const q = useWebhookView(connectionId);
  const [editing, setEditing] = useState(false);
  const v = q.data;
  return (
    <div className="space-y-2" data-testid="webhook-card-line">
      {v ? (
        <p className="text-sm text-ink-2">
          Sends to <span className="font-medium text-ink">{v.host}</span>. Limit: <span data-testid="webhook-limit">{effectiveLimitSentence(v.effective_rate)}</span>.
        </p>
      ) : null}
      <p className="text-sm text-ink-2" data-testid="webhook-ask-line">
        An agent&apos;s send waits for your approval unless you allow it in the account&apos;s access settings.
      </p>
      <div className="flex flex-wrap gap-2">
        <Button type="button" size="sm" variant="outline" onClick={() => setEditing(true)} aria-label={`Edit ${name}`}>
          Edit
        </Button>
      </div>
      {/* Always mounted: the dialog's content unmounts itself on close (its state, and any one-time secret, go with it). */}
      <WebhookEditDialog connectionId={connectionId} name={name} open={editing} onOpenChange={setEditing} />
    </div>
  );
}
