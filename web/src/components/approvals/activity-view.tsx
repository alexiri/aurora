"use client";

import Link from "next/link";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { useQueryClient } from "@tanstack/react-query";

import { ACTIVITY_KIND, ACTIVITY_KIND_FILTER, ACTIVITY_OUTCOME, GATEWAY_DECISION, label, timelineOutcomeGroup, type ActivityKind, type ActivityOutcome } from "@/lib/labels";
import { useAltitude } from "@/lib/altitude";
import { MAX_LIVE_AGENTS, useWorkspaceTimeline } from "@/lib/api/workspace-timeline";
import { When } from "@/components/ui/when";
import { EmptyState, ErrorState, Loading } from "@/components/states";
import { ActivityRows, TimelineLive, useAtTop, type LiveState } from "./activity-timeline";
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
  const [kind, setKind] = useState<ActivityKind | "">("");
  const [outcome, setOutcome] = useState<ActivityOutcome | "">("");
  const [decision, setDecision] = useState("");
  const [hostInput, setHostInput] = useState("");
  const [host, setHost] = useState("");
  const agents = useAgents(wsId);
  const q = useActivity(wsId, { agent, decision, host });
  const items = q.data?.pages.flatMap((p) => p.items) ?? [];
  const byId = new Map(agents.data?.items.map((a) => [a.id, a]) ?? []);

  // (P5-REFINE-16 part B, D-415) The sentence list is the agents' timelines merged and live: one feed per agent (the chosen
  // one, or every agent of the workspace), a stream for the first few, "3 new · Show" while the page is scrolled.
  const agentIds = useMemo(() => (agent ? [agent] : (agents.data?.items ?? []).map((a) => a.id)), [agent, agents.data]);
  const filters = useMemo(() => ({ kinds: kind ? [...ACTIVITY_KIND_FILTER[kind]] : undefined }), [kind]);
  const tl = useWorkspaceTimeline(agentIds, filters, agents.isSuccess);
  const shown = tl.items.filter((i) => !outcome || timelineOutcomeGroup(i) === outcome);
  const atTop = useAtTop();
  const technical = useAltitude().allows("expert");
  const qc = useQueryClient();
  const refresh = useRef<ReturnType<typeof setTimeout> | null>(null);
  // A new request in the Technical view: read the table again (the event has no path; the activity route has its own checks).
  const onRequest = useCallback(() => {
    if (refresh.current) return;
    refresh.current = setTimeout(() => {
      refresh.current = null;
      void qc.invalidateQueries({ queryKey: ["activity", wsId] });
    }, 1000);
  }, [qc, wsId]);
  useEffect(() => () => {
    if (refresh.current) clearTimeout(refresh.current);
  }, []);
  const [live, setLive] = useState<Record<string, LiveState>>({});
  const report = useCallback((id: string, s: LiveState) => setLive((l) => (l[id]?.pending === s.pending && l[id]?.show === s.show ? l : { ...l, [id]: s })), []);
  const waiting = Object.values(live).reduce((n, s) => n + s.pending, 0);
  const showNew = () => {
    Object.values(live).forEach((s) => s.show?.());
    window.scrollTo({ top: 0 });
  };
  const agentBase = (id: string) => `${base}/agents/${byId.get(id)?.handle ?? id}`;
  return (
    <AltitudeScope>
    <div className="max-w-page space-y-4">
      <PageHeader title="Activity" description="What your agents did on the internet and through your apps." className="mb-2" />
      <div role="search" className="flex flex-wrap gap-3">
        <Select aria-label="Kind" value={kind} className="w-48 max-w-full" onChange={(v) => setKind(v as ActivityKind | "")} options={Object.entries(ACTIVITY_KIND).map(([v, e]) => ({ value: v, label: e.label }))} />
        <Select id="act-agent" aria-label="Agent" value={agent} className="w-48 max-w-full" placeholder="Any agent" onChange={setAgent} options={agents.data?.items.map((a) => ({ value: a.id, label: a.name }))} />
        <Select aria-label="Outcome" value={outcome} className="w-48 max-w-full" onChange={(v) => setOutcome(v as ActivityOutcome | "")} options={Object.entries(ACTIVITY_OUTCOME).map(([v, e]) => ({ value: v, label: e.label }))} />
      </div>
      {agentIds.slice(0, MAX_LIVE_AGENTS).map((id) => (
        <TimelineLive key={id} agentId={id} atTop={atTop} technical={technical} onRequest={onRequest} report={report} />
      ))}
      {waiting > 0 ? (
        <div className="sticky top-2 z-10 flex justify-center" role="status">
          <Button size="sm" variant="primary" onClick={showNew}>
            {waiting} new · Show
          </Button>
        </div>
      ) : null}
      {agents.isPending || tl.loading ? <Loading /> : null}
      {tl.error ? <ErrorState error={tl.error} onRetry={tl.refetch} /> : null}
      {agents.isSuccess && !tl.loading && !tl.error && tl.items.length === 0 ? <EmptyState title="No activity" /> : null}
      {tl.items.length > 0 && shown.length === 0 ? <EmptyState title="Nothing matches these filters" /> : null}
      {shown.length > 0 ? <ActivityRows items={shown} base={base} agentBase={agentBase} /> : null}
      {tl.hasMore ? (
        <Button type="button" variant="outline" onClick={() => void tl.loadMore()}>
          Load more
        </Button>
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
        {/* (P5-REFINE-20, workspace-2) Five columns that fit the card at 1256 px: the rule sits under the decision it made and the app / tool under the request. */}
        <DataTable caption="Request activity" stackBelow={900} fit>
          <thead>
            <tr>
              <Th>Request</Th>
              <Th>When</Th>
              <Th>Agent</Th>
              <Th>Model</Th>
              <Th>Decision</Th>
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
                    {e.app_id ? (
                      <span className="mt-0.5 block font-sans text-xs text-ink-2" data-testid="app-tool">
                        App: {e.app_id}
                        {e.tool_id ? ` / ${e.tool_id}` : ""}
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
                    {e.rule_name ? (
                      <span className="mt-0.5 block text-xs text-ink-2" data-testid="rule-name">
                        Rule: {e.rule_name}
                      </span>
                    ) : null}
                  </Td>
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
