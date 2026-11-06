"use client";

import { useParams, useRouter } from "next/navigation";
import { useState } from "react";

import { ErrorState, Loading } from "@/components/states";
import { useAgents, useResolve } from "@/lib/api/queries";
import { cn } from "@/lib/utils";

import { ConnectTab } from "./connect-tab";
import { DeleteZone } from "./delete-zone";
import { InstructionsTab } from "./instructions-tab";

export function AgentPage() {
  const { org, ws, handle } = useParams<{ org: string; ws: string; handle: string }>();
  const router = useRouter();
  const resolved = useResolve(org, ws);
  const wsId = resolved.data?.workspace?.id;
  const agents = useAgents(wsId);
  const [tab, setTab] = useState<"instructions" | "connect">("instructions");

  if (resolved.isPending || (wsId && agents.isPending)) return <Loading />;
  if (resolved.isError) return <ErrorState error={resolved.error} onRetry={() => resolved.refetch()} />;
  if (agents.isError) return <ErrorState error={agents.error} onRetry={() => agents.refetch()} />;
  const agent = agents.data?.items.find((a) => a.handle === handle);
  if (!agent) return <ErrorState error={{ code: "not_found" }} />;

  const byo = agent.kind === "byo";
  const tabs = [
    { id: "instructions" as const, label: "Instructions" },
    ...(byo ? [{ id: "connect" as const, label: "Computer: Connect" }] : []),
  ];

  return (
    <div className="max-w-4xl space-y-4">
      <h1 className="text-xl font-semibold">
        {agent.name} <span className="font-mono text-sm text-neutral-600">@{agent.handle}</span>
      </h1>
      <div role="tablist" aria-label="Agent" className="flex gap-1 border-b border-neutral-200">
        {tabs.map((t) => (
          <button
            key={t.id}
            role="tab"
            type="button"
            id={`tab-${t.id}`}
            aria-selected={tab === t.id}
            aria-controls="agent-panel"
            onClick={() => setTab(t.id)}
            className={cn(
              "-mb-px border-b-2 px-3 py-2 text-sm",
              tab === t.id ? "border-neutral-900 font-medium" : "border-transparent",
            )}
          >
            {t.label}
          </button>
        ))}
        <button
          role="tab"
          type="button"
          disabled
          aria-disabled="true"
          title="Available once grants are enabled"
          className="-mb-px cursor-not-allowed border-b-2 border-transparent px-3 py-2 text-sm text-neutral-400"
        >
          Access
        </button>
      </div>
      <div role="tabpanel" id="agent-panel" aria-labelledby={`tab-${tab}`} className="pt-2">
        {tab === "instructions" ? (
          <InstructionsTab key={agent.updated_at} agent={agent} wsId={wsId!} />
        ) : null}
        {tab === "connect" && byo ? (
          <ConnectTab agent={agent} />
        ) : null}
        {tab === "instructions" ? (
          <div className="pt-6">
            <DeleteZone agentId={agent.id} handle={agent.handle} wsId={wsId!} onDeleted={() => router.replace(`/o/${org}/w/${ws}/agents`)} />
          </div>
        ) : null}
      </div>
    </div>
  );
}
