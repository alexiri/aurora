"use client";

import Link from "next/link";
import { useState } from "react";

import {
  ACTIVITY_KIND,
  ACTIVITY_OUTCOME,
  GATEWAY_DECISION,
  activityKind,
  activityOutcome,
  activitySentence,
  activityTone,
  label,
} from "@/lib/labels";
import { productName } from "@/lib/api/app-name";
import { useApps } from "@/lib/api/apps";
import { StatusDot } from "@/components/ui/status-dot";
import { When } from "@/components/ui/when";
import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Altitude, AltitudeScope, MoreControlsLine } from "@/components/ui/altitude";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { DataTable, Td, Th } from "@/components/ui/data-table";
import { PageHeader } from "@/components/ui/page-header";
import { Field, inputClass } from "@/components/ui/field";
import { LoadMore } from "@/components/ui/load-more";
import { Select } from "@/components/ui/select";
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

export function decisionTone(d: string): "accent" | "neutral" | "danger" {
  if (d === "allow" || d === "approved") return "accent";
  if (d === "approval_expired" || d === "error") return "neutral";
  if (d === "blocked" || d.endsWith("_denied") || d.endsWith("_refused") || d === "rate_limited") return "danger";
  return "neutral";
}

/** (P5-AGENT-2) The agent filter is in the URL (`?agent=<id>`), so the agent page's "All activity" lands filtered. */
function agentFromUrl(): string {
  return typeof window === "undefined" ? "" : (new URLSearchParams(window.location.search).get("agent") ?? "");
}

export function ActivityView({ wsId, base }: { wsId: string; base: string }) {
  const [agent, setAgentState] = useState(agentFromUrl);
  const setAgent = (id: string) => {
    setAgentState(id);
    const url = new URL(window.location.href);
    if (id) url.searchParams.set("agent", id);
    else url.searchParams.delete("agent");
    window.history.replaceState(window.history.state, "", `${url.pathname}${url.search}${url.hash}`);
  };
  const [kind, setKind] = useState("");
  const [outcome, setOutcome] = useState("");
  const [open, setOpen] = useState<string | null>(null);
  const [decision, setDecision] = useState("");
  const [hostInput, setHostInput] = useState("");
  const [host, setHost] = useState("");
  const agents = useAgents(wsId);
  const q = useActivity(wsId, { agent, decision, host });
  const items = q.data?.pages.flatMap((p) => p.items) ?? [];
  const byId = new Map(agents.data?.items.map((a) => [a.id, a]) ?? []);
  const apps = useApps(wsId);
  const appNames = new Map((apps.data?.items ?? []).map((a) => [a.app_id, productName(a)]));
  const shown = items.filter((e) => (!kind || activityKind(e) === kind) && (!outcome || activityOutcome(e) === outcome));
  return (
    <AltitudeScope>
    <div className="max-w-page space-y-4">
      <PageHeader title="Activity" description="What your agents did on the internet and through your apps." className="mb-2" />
      <div role="search" className="flex flex-wrap gap-3">
        <Select aria-label="Kind" value={kind} className="w-48 max-w-full" onChange={setKind} options={Object.entries(ACTIVITY_KIND).map(([v, e]) => ({ value: v, label: e.label }))} />
        <Select id="act-agent" aria-label="Agent" value={agent} className="w-48 max-w-full" placeholder="Any agent" onChange={setAgent} options={agents.data?.items.map((a) => ({ value: a.id, label: a.name }))} />
        <Select aria-label="Outcome" value={outcome} className="w-48 max-w-full" onChange={setOutcome} options={Object.entries(ACTIVITY_OUTCOME).map(([v, e]) => ({ value: v, label: e.label }))} />
      </div>
      {q.isPending ? <Loading /> : null}
      {q.isError ? <ErrorState error={q.error} onRetry={() => q.refetch()} /> : null}
      {q.data && items.length === 0 ? <EmptyState title="No activity" /> : null}
      {items.length > 0 && shown.length === 0 ? <EmptyState title="Nothing matches these filters" /> : null}
      {shown.length > 0 ? (
        <ul className="divide-y divide-line overflow-hidden rounded-card border border-line bg-surface shadow-card" data-testid="activity-sentences">
          {shown.map((e) => {
            const a = byId.get(e.agent_id);
            const words = activitySentence(e, a?.name ?? "An agent", e.app_id ? (appNames.get(e.app_id) ?? e.app_id) : null);
            const raw = `${e.method} ${e.host}${e.path}`;
            return (
              <li key={e.id} className="space-y-2 px-4 py-3">
                <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
                  <StatusDot tone={activityTone(e)}>
                    <span className="sr-only">{label(ACTIVITY_OUTCOME, activityOutcome(e))}</span>
                  </StatusDot>
                  <div className="min-w-0 flex-1 basis-56">
                    <p className="text-sm font-medium text-ink">{words.title}</p>
                    <p className="text-xs text-muted">
                      {words.meta === "Waiting for you in Approvals" ? <Link className="text-link underline" href={`${base}/approvals`}>{words.meta}</Link> : words.meta}
                    </p>
                  </div>
                  <When value={e.ts} className="text-sm text-muted" />
                  <Button size="sm" variant="ghost" aria-expanded={open === e.id} onClick={() => setOpen(open === e.id ? null : e.id)}>
                    Details
                  </Button>
                </div>
                {open === e.id ? (
                  <div className="rounded-control bg-canvas p-3" data-testid="activity-raw">
                    <p className="text-xs font-medium uppercase tracking-wide text-muted">Details · the raw request</p>
                    <p className="mt-1 break-all font-mono text-xs text-ink-2">
                      {raw} · {e.decision} · {e.status ?? "no status"}
                    </p>
                  </div>
                ) : null}
              </li>
            );
          })}
        </ul>
      ) : null}
      {/* (P5-REFINE-3) Expert: the request log with its decision vocabulary. Below it, one collapsed line. */}
      {items.length > 0 ? (
        <Altitude id="log.requests" level="expert" label="Technical view: every request, with the rule that decided it">
        <form
          role="search"
          aria-label="Filter the technical view"
          className="mb-3 flex flex-wrap items-start gap-x-4 gap-y-3 rounded-card border border-line bg-surface p-4 shadow-card"
          onSubmit={(e) => {
            e.preventDefault();
            setHost(hostInput.trim());
          }}
        >
          <Field id="act-decision" label="Decision">
            <Select id="act-decision" value={decision} className="w-52 max-w-full" onChange={setDecision} options={DECISIONS.map((d) => ({ value: d, label: d ? label(GATEWAY_DECISION, d) : "All decisions" }))} />
          </Field>
          <Field id="act-host" label="Host">
            <input id="act-host" className={cn(inputClass, "w-56 max-w-full font-mono")} placeholder="api.example.com" value={hostInput} onChange={(e) => setHostInput(e.target.value)} />
          </Field>
          <Button type="submit" variant="outline" className="sm:mt-label-offset">
            Filter
          </Button>
        </form>
        <DataTable caption="Request activity" stackBelow={900}>
          <thead>
            <tr>
              <Th>Request</Th>
              <Th>When</Th>
              <Th>Agent</Th>
              <Th>Model</Th>
              <Th>Decision</Th>
              <Th>Rule</Th>
              <Th>App / tool</Th>
            </tr>
          </thead>
          <tbody>
            {items.map((e) => {
              const a = byId.get(e.agent_id);
              return (
                <tr key={e.id}>
                  <Td cell="title" className="break-all font-mono text-xs">
                    {e.method} {e.host}
                    {e.path}
                    {e.context_label ? (
                      <span className="mt-0.5 block font-sans text-xs text-muted" data-testid="context-label">
                        {e.context_label}
                      </span>
                    ) : null}
                  </Td>
                  <Td label="When" className="whitespace-nowrap text-ink-2">
                    <When value={e.ts} />
                  </Td>
                  <Td label="Agent">
                    {a ? (
                      <Link className="text-link underline" href={`${base}/agents/${a.handle}`}>
                        {a.name}
                      </Link>
                    ) : (
                      "—"
                    )}
                  </Td>
                  <Td label="Model" className="break-all font-mono text-xs" data-testid="llm-model">
                    {e.llm_model ?? "—"}
                  </Td>
                  <Td>
                    <Badge tone={decisionTone(e.decision)}>{label(GATEWAY_DECISION, e.decision)}</Badge>
                    {e.status ? <span className="ml-1.5 text-xs text-muted">{e.status}</span> : null}
                  </Td>
                  <Td label="Rule">{e.rule_name ?? "—"}</Td>
                  <Td label="App / tool">{e.app_id ? `${e.app_id}${e.tool_id ? ` / ${e.tool_id}` : ""}` : "—"}</Td>
                </tr>
              );
            })}
          </tbody>
        </DataTable>
        <LoadMore query={q} />
        </Altitude>
      ) : null}
      <MoreControlsLine />
    </div>
    </AltitudeScope>
  );
}
