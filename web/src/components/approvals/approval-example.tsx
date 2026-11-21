"use client";

import { useState } from "react";

import { Fold } from "@/components/ui/fold";
import type { Approval, Decision } from "@/lib/api/approvals";

import { ApprovalQuestion } from "./approval-question";

/** What each answer would do, in one line, under the example card (workspace-11). */
export function exampleExplanation(agent: string, decision: Decision, always: boolean): string {
  if (decision === "deny") return `${agent} is told no, and carries on without it.`;
  return always
    ? `${agent} does this kind of thing without asking from now on. You can undo it in Apps & accounts.`
    : `${agent} does it this time, and asks again next time.`;
}

/**
 * (P5-REFINE-16, workspace-11, rec 10) What an approval looks like, for a workspace where none is waiting. The sample is built
 * here and goes nowhere: no request, no countdown, no Live pill, never counted. Its three answers each explain what they would
 * do and change nothing.
 */
export function ApprovalExample({ agentName, org, ws }: { agentName: string; org: string; ws: string }) {
  const [said, setSaid] = useState<string | null>(null);
  // The issue number is written with a unicode escape: the colour guard would read a bare "hash 214" as a hex colour.
  const sample: Approval = {
    id: "example",
    org_id: "example",
    workspace_id: "example",
    status: "pending",
    method: "POST",
    host: "api.github.com",
    path: "/repos/acme/web/issues/214",
    created_at: "2026-01-01T00:00:00Z",
    deadline: "2026-01-01T00:00:00Z",
    parked: false,
    can_always_allow: true,
    sentence: {
      agent: { id: "example", name: agentName },
      action: "close issue \u0023214",
      app: { app_id: "github", name: "GitHub" },
    },
    summary: {
      source: "generic",
      title: "Close issue \u0023214",
      text: "Acme/web · “Login page flickers” · it says the fix is merged",
      fields: [],
    },
  } as Approval;
  return (
    <Fold summary="What an approval looks like" data-testid="approval-example">
      <section aria-label="Example approval" className="mt-2 space-y-3">
        <p className="text-ink-2">This is what {agentName} asks before it changes something. Try the buttons: nothing is sent.</p>
        <div className="rounded-card border border-line bg-surface p-4 shadow-card">
          <ApprovalQuestion
            approval={sample}
            now={0}
            pending={false}
            canApprove
            org={org}
            ws={ws}
            showExpiry={false}
            showRequest={false}
            onDecide={(d, always) => setSaid(exampleExplanation(agentName, d, always === true))}
          />
          {said ? (
            <p role="status" className="mt-3 text-sm text-ink-2" data-testid="example-says">
              {said}
            </p>
          ) : null}
        </div>
      </section>
    </Fold>
  );
}
