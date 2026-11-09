"use client";

import { useState } from "react";

import { Button } from "@/components/ui/button";
import { Notice } from "@/components/ui/notice";
import { useApproveConnector, type ConnectorAdmin } from "@/lib/api/connectors";

import { ConnectorError } from "./shared";
import { ManifestDiff, ManifestSummary } from "./manifest-views";

/** A pending (new or widened) manifest: reasons, the diff, the summary, and Approve of that exact digest. */
export function ApprovalPanel({ c }: { c: ConnectorAdmin }) {
  const approve = useApproveConnector();
  const [reviewed, setReviewed] = useState(false);
  return (
    <Notice tone="pending" className="space-y-3">
      <h4 className="font-medium">Manifest awaiting approval</h4>
      {c.widening.length ? (
        <ul aria-label="Widening reasons" className="list-disc pl-5">
          {c.widening.map((w, i) => (
            <li key={i}>{w}</li>
          ))}
        </ul>
      ) : (
        <p>First registration.</p>
      )}
      {c.approved_manifest ? <ManifestDiff approved={c.approved_manifest} manifest={c.manifest} /> : null}
      <ManifestSummary manifest={c.manifest} />
      <label className="flex items-center gap-2 font-medium">
        <input type="checkbox" className="size-4 accent-accent" checked={reviewed} onChange={(e) => setReviewed(e.target.checked)} />
        I have reviewed these capabilities and setup fields
      </label>
      <Button
        variant="approve"
        disabled={!reviewed || !c.manifest_digest || approve.isPending}
        onClick={() => {
          approve.mutate(
            { id: c.id, manifest_digest: c.manifest_digest! },
            // A changed manifest must be reviewed again.
            { onError: () => setReviewed(false) },
          );
        }}
      >
        Approve manifest
      </Button>
      <ConnectorError error={approve.error} />
    </Notice>
  );
}
