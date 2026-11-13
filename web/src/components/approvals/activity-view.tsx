"use client";

import Link from "next/link";
import { useState } from "react";

import { fmtDate } from "@/components/account/key-table";
import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { DataTable, Td, Th } from "@/components/ui/data-table";
import { PageHeader } from "@/components/ui/page-header";
import { Field, inputClass } from "@/components/ui/field";
import { cn } from "@/lib/utils";
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
    <div className="max-w-page space-y-4">
      <PageHeader
        title="Activity"
        description="Every request your agents made through the gateway, and what the policy decided."
        className="mb-2"
      />
      <form
        role="search"
        className="flex flex-wrap items-start gap-x-4 gap-y-3 rounded-card border border-line bg-surface p-4 shadow-card"
        onSubmit={(e) => {
          e.preventDefault();
          setHost(hostInput.trim());
        }}
      >
        <Field id="act-agent" label="Agent">
          <select
            id="act-agent"
            className={cn(inputClass, "w-48 max-w-full")}
            value={agent}
            onChange={(e) => setAgent(e.target.value)}
          >
            <option value="">All agents</option>
            {agents.data?.items.map((a) => (
              <option key={a.id} value={a.id}>
                {a.name}
              </option>
            ))}
          </select>
        </Field>
        <Field id="act-decision" label="Decision">
          <select
            id="act-decision"
            className={cn(inputClass, "w-52 max-w-full")}
            value={decision}
            onChange={(e) => setDecision(e.target.value)}
          >
            {DECISIONS.map((d) => (
              <option key={d} value={d}>
                {d || "All decisions"}
              </option>
            ))}
          </select>
        </Field>
        <Field id="act-host" label="Host">
          <input
            id="act-host"
            className={cn(inputClass, "w-56 max-w-full font-mono")}
            placeholder="api.example.com"
            value={hostInput}
            onChange={(e) => setHostInput(e.target.value)}
          />
        </Field>
        <Button type="submit" variant="outline" className="sm:mt-label-offset">
          Filter
        </Button>
      </form>
      {q.isPending ? <Loading /> : null}
      {q.isError ? <ErrorState error={q.error} onRetry={() => q.refetch()} /> : null}
      {q.data && items.length === 0 ? <EmptyState title="No activity" /> : null}
      {items.length > 0 ? (
        <DataTable caption="Request activity">
          <thead>
            <tr>
              <Th>When</Th>
              <Th>Agent</Th>
              <Th>Request</Th>
              <Th>Decision</Th>
              <Th>Rule</Th>
              <Th>App / tool</Th>
              <Th>Status</Th>
            </tr>
          </thead>
          <tbody>
            {items.map((e) => {
              const a = byId.get(e.agent_id);
              return (
                <tr key={e.id}>
                  <Td className="whitespace-nowrap text-ink-2">{fmtDate(e.ts)}</Td>
                  <Td>
                    {a ? (
                      <Link className="text-link underline" href={`${base}/agents/${a.handle}`}>
                        {a.name}
                      </Link>
                    ) : (
                      "—"
                    )}
                  </Td>
                  <Td className="font-mono text-xs">
                    {e.method} {e.host}
                    {e.path}
                    {e.context_label ? (
                      <span className="mt-0.5 block font-sans text-xs text-muted" data-testid="context-label">
                        {e.context_label}
                      </span>
                    ) : null}
                  </Td>
                  <Td>
                    <Badge tone={decisionTone(e.decision)}>{e.decision}</Badge>
                  </Td>
                  <Td>{e.rule_name ?? "—"}</Td>
                  <Td>{e.app_id ? `${e.app_id}${e.tool_id ? ` / ${e.tool_id}` : ""}` : "—"}</Td>
                  <Td>{e.status}</Td>
                </tr>
              );
            })}
          </tbody>
        </DataTable>
      ) : null}
      {q.hasNextPage ? (
        <Button variant="outline" disabled={q.isFetchingNextPage} onClick={() => void q.fetchNextPage()}>
          Load more
        </Button>
      ) : null}
    </div>
  );
}
