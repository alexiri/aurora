"use client";

import { useState } from "react";

import { ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { ErrorNotice } from "@/components/ui/notice";
import { When } from "@/components/ui/when";
import { canRedact, useMemoryRevisions, useRedactRevision, useRestoreMemory, type Memory } from "@/lib/api/memory";

export function RevisionHistory({ agentId, memory }: { agentId: string; memory: Memory }) {
  const revs = useMemoryRevisions(agentId, memory.key, true);
  const restore = useRestoreMemory(agentId, memory.key);
  const redact = useRedactRevision(agentId, memory.key);
  const [viewing, setViewing] = useState<number | null>(null);
  const [confirmRedact, setConfirmRedact] = useState<number | null>(null);
  const failed = restore.error ?? redact.error;

  return (
    <Card role="region" aria-label="Revision history" className="space-y-3">
      <h3 className="font-display text-base font-semibold text-ink">Revision history</h3>
      {revs.isPending ? <Loading /> : null}
      {revs.isError ? <ErrorState error={revs.error} onRetry={() => revs.refetch()} /> : null}
      <ErrorNotice error={failed} />
      <ul className="divide-y divide-line">
        {revs.data?.items.map((r) => (
          <li key={r.revision} className="space-y-1 py-3 text-sm first:pt-0 last:pb-0">
            <p>
              <strong>#{r.revision}</strong>
              {r.revision === memory.revision ? " (latest)" : ""} - {r.author_label} ({r.author_kind}, {r.provenance}) -{" "}
              <When value={r.created_at} />
            </p>
            {r.redacted ? <p className="text-muted">Content removed</p> : null}
            {viewing === r.revision && r.content != null ? (
              <pre className="mt-1 max-h-48 overflow-auto whitespace-pre-wrap rounded-control border border-line bg-subtle p-2 font-mono text-xs text-ink">{r.content}</pre>
            ) : null}
            <div className="mt-1 flex flex-wrap gap-2">
              {!r.redacted ? (
                <Button type="button" size="sm" variant="ghost" onClick={() => setViewing(viewing === r.revision ? null : r.revision)}>
                  {viewing === r.revision ? "Hide" : "View"} #{r.revision}
                </Button>
              ) : null}
              {!r.redacted && r.revision !== memory.revision ? (
                <Button type="button" size="sm" variant="outline" disabled={restore.isPending} onClick={() => restore.mutate(r.revision)}>
                  Restore #{r.revision}
                </Button>
              ) : null}
              {!r.redacted ? (
                confirmRedact === r.revision ? (
                  <>
                    <Button type="button" size="sm" variant="outline" onClick={() => void redact.mutateAsync(r.revision).then(() => setConfirmRedact(null))}>
                      Confirm removing content of #{r.revision}
                    </Button>
                    <Button type="button" size="sm" variant="ghost" onClick={() => setConfirmRedact(null)}>Cancel</Button>
                  </>
                ) : (
                  <Button
                    type="button"
                    size="sm"
                    variant="outline"
                    disabled={!canRedact(r, memory.revision)}
                    title={r.revision === memory.revision ? "The content of the latest revision cannot be removed. Save a new version first." : undefined}
                    onClick={() => setConfirmRedact(r.revision)}
                  >
                    Remove content of #{r.revision}
                  </Button>
                )
              ) : null}
            </div>
          </li>
        ))}
      </ul>
    </Card>
  );
}
