"use client";

import Link from "next/link";
import { useState } from "react";

import { fmtDate } from "@/components/account/key-table";
import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { PageHeader } from "@/components/ui/page-header";
import { inputClass } from "@/components/ui/field";
import { useActivity } from "@/lib/api/approvals";
import { useAgents } from "@/lib/api/queries";

// request_log.decision vocabulary (docs/03-data-model.md §request_log).
const DECISIONS = [
  "",
  "allow",
  "approved",
  "blocked",
  "default_denied",
  "egress_denied",
  "governance_refused",
  "rate_limited",
  "approval_denied",
  "approval_expired",
  "guard_refused",
  "error",
];

export function decisionTone(d: string): "accent" | "pending" | "danger" {
  if (d === "allow" || d === "approved") return "accent";
  if (d === "approval_expired" || d === "error") return "pending";
  if (d === "blocked" || d.endsWith("_denied") || d.endsWith("_refused") || d === "rate_limited") return "danger";
  return "pending";
}

export function ActivityView({ wsId, base }: { wsId: string; base: string }) {
  const [agent, setAgent] = useState("");
  const [decision, setDecision] = useState("");
  const [hostInput, setHostInput] = useState("");
  const [host, setHost] = useState("");
  const agents = useAgents(wsId);
  const q = useActivity(wsId, { agent, decision, host });
  const items = q.data?.pages.flatMap((p) => p.items) ?? [];
  const byId = new Map(agents.data?.items.map((a) => [a.id, a]) ?? []);
  return (
    <div className="max-w-6xl space-y-4">
      <PageHeader title="Activity" className="mb-2" />
      <form
        role="search"
        className="flex flex-wrap items-end gap-2"
        onSubmit={(e) => {
          e.preventDefault();
          setHost(hostInput.trim());
        }}
      >
        <label className="text-sm">
          Agent
          <select className={inputClass} value={agent} onChange={(e) => setAgent(e.target.value)}>
            <option value="">All agents</option>
            {agents.data?.items.map((a) => (
              <option key={a.id} value={a.id}>
                {a.name}
              </option>
            ))}
          </select>
        </label>
        <label className="text-sm">
          Decision
          <select className={inputClass} value={decision} onChange={(e) => setDecision(e.target.value)}>
            {DECISIONS.map((d) => (
              <option key={d} value={d}>
                {d || "All decisions"}
              </option>
            ))}
          </select>
        </label>
        <label className="text-sm">
          Host
          <input
            className={inputClass}
            placeholder="api.example.com"
            value={hostInput}
            onChange={(e) => setHostInput(e.target.value)}
          />
        </label>
        <Button type="submit" variant="outline">
          Filter
        </Button>
      </form>
      {q.isPending ? <Loading /> : null}
      {q.isError ? <ErrorState error={q.error} onRetry={() => q.refetch()} /> : null}
      {q.data && items.length === 0 ? <EmptyState title="No activity" /> : null}
      {items.length > 0 ? (
        <div className="overflow-x-auto">
          <table className="w-full text-left text-sm">
            <thead className="text-muted">
              <tr>
                <th className="py-2 pr-4 font-medium">When</th>
                <th className="py-2 pr-4 font-medium">Agent</th>
                <th className="py-2 pr-4 font-medium">Request</th>
                <th className="py-2 pr-4 font-medium">Decision</th>
                <th className="py-2 pr-4 font-medium">Rule</th>
                <th className="py-2 pr-4 font-medium">App / tool</th>
                <th className="py-2 pr-4 font-medium">Status</th>
              </tr>
            </thead>
            <tbody>
              {items.map((e) => {
                const a = byId.get(e.agent_id);
                return (
                  <tr key={e.id} className="border-t border-line">
                    <td className="py-2 pr-4">{fmtDate(e.ts)}</td>
                    <td className="py-2 pr-4">
                      {a ? (
                        <Link className="text-link underline"href={`${base}/agents/${a.handle}`}>
                          {a.name}
                        </Link>
                      ) : (
                        "—"
                      )}
                    </td>
                    <td className="py-2 pr-4 font-mono">
                      {e.method} {e.host}
                      {e.path}
                      {e.context_label ? (
                        <span className="mt-0.5 block font-sans text-xs text-muted" data-testid="context-label">
                          {e.context_label}
                        </span>
                      ) : null}
                    </td>
                    <td className="py-2 pr-4">
                      <Badge tone={decisionTone(e.decision)}>{e.decision}</Badge>
                    </td>
                    <td className="py-2 pr-4">{e.rule_name ?? "—"}</td>
                    <td className="py-2 pr-4">{e.app_id ? `${e.app_id}${e.tool_id ? ` / ${e.tool_id}` : ""}` : "—"}</td>
                    <td className="py-2 pr-4">{e.status}</td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      ) : null}
      {q.hasNextPage ? (
        <Button variant="outline" disabled={q.isFetchingNextPage} onClick={() => void q.fetchNextPage()}>
          Load more
        </Button>
      ) : null}
    </div>
  );
}
