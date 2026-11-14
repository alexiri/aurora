"use client";

// The standalone terminal window (FIX-138): `/o/<org>/w/<ws>/agents/<agent>/terminal`, opened by the Terminal page's
// "Open in new window" button. No app chrome: a title bar and a pane that fills the viewport. Same session cookie, same
// `GET /v1/agents/{agent}/terminal` socket and single-use grant as the tab (no new auth path); it is a second,
// independent shell that counts against the sandbox's leg cap.
import { useParams } from "next/navigation";
import { useEffect, useRef } from "react";

import { ErrorState, Loading } from "@/components/states";
import { Notice } from "@/components/ui/notice";
import { useAgents, useInstance, useResolve } from "@/lib/api/queries";

import { TerminalPane } from "./terminal-tab";

export function TerminalWindow() {
  const { org, ws, handle } = useParams<{ org: string; ws: string; handle: string }>();
  const resolved = useResolve(org, ws);
  const wsId = resolved.data?.workspace?.id;
  const agents = useAgents(wsId);
  const instance = useInstance();
  const titleRef = useRef<HTMLHeadingElement>(null);
  const agent = agents.data?.items.find((a) => a.handle === handle);
  useEffect(() => {
    document.title = `${agent?.name ?? handle} — Terminal`;
  }, [agent?.name, handle]);

  if (resolved.isPending || (wsId && agents.isPending) || instance.isPending) return <Loading />;
  if (resolved.isError) return <ErrorState error={resolved.error} onRetry={() => resolved.refetch()} />;
  if (agents.isError) return <ErrorState error={agents.error} onRetry={() => agents.refetch()} />;
  if (!agent) return <ErrorState error={{ code: "not_found" }} />;
  if (agent.kind === "byo" || instance.data?.features.terminal !== true) {
    return (
      <div className="p-4">
        <Notice tone="neutral" role="status">
          The terminal is not available for this agent.
        </Notice>
      </div>
    );
  }
  return (
    <main className="flex min-h-screen flex-col bg-canvas">
      <header className="flex items-center gap-3 border-b border-line bg-surface px-4 py-2">
        <h1 ref={titleRef} tabIndex={-1} className="min-w-0 truncate font-display text-base font-semibold text-ink">
          {agent.name} — Terminal
        </h1>
        <span className="font-mono text-xs text-muted">@{agent.handle}</span>
        <p className="ml-auto hidden text-xs text-muted sm:block">Esc, then Tab leaves the terminal.</p>
      </header>
      <div className="flex-1 p-2">
        <TerminalPane agentId={agent.id} frozen={agent.frozen !== null} variant="window" exitFocusRef={titleRef} />
      </div>
    </main>
  );
}
