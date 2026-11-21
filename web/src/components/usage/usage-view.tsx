"use client";

import Link from "next/link";

import { formatExact } from "@/lib/format-date";
import { useEffect, useRef, useState, type ReactNode } from "react";

import { EmptyState, ErrorState, Loading } from "@/components/states";
import { BarChart } from "@/components/ui/bar-chart";
import { DataTable, Td, Th } from "@/components/ui/data-table";
import { DateField } from "@/components/ui/date-field";
import { Field } from "@/components/ui/field";
import { FilterChip } from "@/components/ui/filter-chip";
import { Segmented } from "@/components/ui/segmented";
import { StatTile } from "@/components/ui/stat-tile";
import { Button } from "@/components/ui/button";
import { Select } from "@/components/ui/select";
import { errorMessage } from "@/lib/api/errors";
import { useAdminOrgs } from "@/lib/api/freeze";
import { useAgents, useWorkspaces } from "@/lib/api/queries";
import { ApiError } from "@/lib/api/client";
import { useInstanceUsage, useOrgUsage, useWorkspaceUsage, type InstanceUsage, type OrgUsage, type UsageCounters, type UsageModel, type WorkspaceUsage } from "@/lib/api/usage";
import {
  RANGE_DAYS,
  compact,
  duration,
  exact,
  groupByWeek,
  measureValue,
  presetRange,
  shortDay,
  type DayPoint,
  type Measure,
  type RangeId,
} from "@/lib/usage-format";

export type UsageLevel =
  | { kind: "workspace"; wsId: string; /** `/o/<org>/w/<ws>` */ base: string }
  | { kind: "org"; orgId: string; orgSlug: string }
  | { kind: "admin" };

const OPENING =
  "How much your agents used: AI tokens, the turns they answered, and how long their computers were awake. Days are in UTC.";
const MIN_BAR_PX = 4;
const CHART_PAD = 52;
const DATE_RE = /^\d{4}-\d{2}-\d{2}$/;

/** The URL state (C-1004): `range=7d|30d|90d` or `from=`/`to=`, and one filter (`agent=`, `workspace=` or `org=`). */
interface UsageParams {
  range: RangeId;
  from: string;
  to: string;
  filter: string;
}

const FILTER_PARAM = { workspace: "agent", org: "workspace", admin: "org" } as const;

function readParams(param: string): UsageParams {
  const q = typeof window === "undefined" ? new URLSearchParams() : new URLSearchParams(window.location.search);
  const r = q.get("range");
  const from = q.get("from") ?? "";
  const to = q.get("to") ?? "";
  const range: RangeId = r === "7d" || r === "30d" || r === "90d" ? r : from || to ? "custom" : "30d";
  return { range, from, to, filter: q.get(param) ?? "" };
}

function useUsageParams(param: string): [UsageParams, (patch: Partial<UsageParams>) => void] {
  const [p, setP] = useState<UsageParams>(() => readParams(param));
  const set = (patch: Partial<UsageParams>) => {
    const next = { ...p, ...patch };
    setP(next);
    const url = new URL(window.location.href);
    const put = (k: string, v: string) => (v ? url.searchParams.set(k, v) : url.searchParams.delete(k));
    if (next.range === "custom") {
      url.searchParams.delete("range");
      put("from", next.from);
      put("to", next.to);
    } else {
      url.searchParams.set("range", next.range);
      url.searchParams.delete("from");
      url.searchParams.delete("to");
    }
    put(param, next.filter);
    window.history.replaceState(window.history.state, "", `${url.pathname}${url.search}${url.hash}`);
  };
  return [p, set];
}

/** The pixel width of an element, so a chart can tell when its bars would be under 4 px. */
function useWidth(): [React.RefObject<HTMLDivElement | null>, number] {
  const ref = useRef<HTMLDivElement>(null);
  const [w, setW] = useState(600);
  useEffect(() => {
    const el = ref.current;
    if (!el || typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(() => {
      const x = Math.floor(el.getBoundingClientRect().width);
      if (x > 0) setW(x);
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  return [ref, w];
}

const MEASURES: { value: Measure; label: string }[] = [
  { value: "tokens", label: "Tokens" },
  { value: "turns", label: "Turns" },
  { value: "turn_time", label: "Turn time" },
  { value: "awake_time", label: "Awake time" },
];

const formatMeasure = (m: Measure) => (v: number) => (m === "turns" ? exact(v) : m === "tokens" ? `${exact(v)} tokens` : duration(v));

const isZero = (c: UsageCounters) =>
  c.turns + c.turn_seconds + c.awake_seconds + c.input_tokens + c.output_tokens + c.cache_read_tokens + c.cache_write_tokens === 0;

/** A table row's name cell and counters. */
interface Row {
  key: string;
  name: ReactNode;
  counters: UsageCounters;
}

function CountersTable({ caption, first, rows }: { caption: string; first: string; rows: Row[] }) {
  return (
    <DataTable caption={caption}>
      <thead>
        <tr>
          <Th>{first}</Th>
          <Th className="text-right">Tokens in</Th>
          <Th className="text-right">Tokens out</Th>
          <Th className="text-right">Cache read</Th>
          <Th className="text-right">Cache write</Th>
          <Th className="text-right">Turns</Th>
          <Th className="text-right">Turn time</Th>
          <Th className="text-right">Awake time</Th>
        </tr>
      </thead>
      <tbody>
        {rows.map((r) => (
          <tr key={r.key}>
            <Td className="max-w-64 break-words">{r.name}</Td>
            <Td className="text-right tabular-nums">{exact(r.counters.input_tokens)}</Td>
            <Td className="text-right tabular-nums">{exact(r.counters.output_tokens)}</Td>
            <Td className="text-right tabular-nums">{exact(r.counters.cache_read_tokens)}</Td>
            <Td className="text-right tabular-nums">{exact(r.counters.cache_write_tokens)}</Td>
            <Td className="text-right tabular-nums">{exact(r.counters.turns)}</Td>
            <Td className="text-right tabular-nums">{duration(r.counters.turn_seconds)}</Td>
            <Td className="text-right tabular-nums">{duration(r.counters.awake_seconds)}</Td>
          </tr>
        ))}
      </tbody>
    </DataTable>
  );
}

function ModelsTable({ models }: { models: UsageModel[] }) {
  return (
    <div className="space-y-2">
      <DataTable caption="Usage by model">
        <thead>
          <tr>
            <Th>Model</Th>
            <Th className="text-right">Tokens in</Th>
            <Th className="text-right">Tokens out</Th>
            <Th className="text-right">Cache read</Th>
            <Th className="text-right">Cache write</Th>
            <Th className="text-right">Turns</Th>
            <Th className="text-right">Turn time</Th>
          </tr>
        </thead>
        <tbody>
          {models.map((m) => (
            <tr key={`${m.provider}/${m.endpoint_id}/${m.model}`}>
              <Td className="max-w-64 break-words">
                {m.model ? (
                  <span className="font-mono text-xs">{m.model}</span>
                ) : (
                  <span className="text-muted">Unknown model</span>
                )}
                {m.provider ? <span className="ml-2 text-xs text-muted">{m.provider}</span> : null}
              </Td>
              <Td className="text-right tabular-nums">{exact(m.input_tokens)}</Td>
              <Td className="text-right tabular-nums">{exact(m.output_tokens)}</Td>
              <Td className="text-right tabular-nums">{exact(m.cache_read_tokens)}</Td>
              <Td className="text-right tabular-nums">{exact(m.cache_write_tokens)}</Td>
              <Td className="text-right tabular-nums">{exact(m.turns)}</Td>
              <Td className="text-right tabular-nums">{duration(m.turn_seconds)}</Td>
            </tr>
          ))}
        </tbody>
      </DataTable>
      <p className="text-xs text-ink-2">
        Turns count under the agent&apos;s main model; a model used only by helpers shows tokens and no turns.
      </p>
    </div>
  );
}

function PerDay({ days }: { days: (DayPoint & UsageCounters)[] }) {
  const [measure, setMeasure] = useState<Measure>("tokens");
  const [table, setTable] = useState(false);
  const [ref, width] = useWidth();
  const byWeek = (width - CHART_PAD) / Math.max(1, days.length) < MIN_BAR_PX;
  const points: DayPoint[] = byWeek ? groupByWeek(days) : days;
  const fmt = formatMeasure(measure);
  const label = MEASURES.find((m) => m.value === measure)?.label ?? "";
  return (
    <section aria-labelledby="usage-day-h" className="space-y-3" ref={ref}>
      <div className="flex flex-wrap items-end justify-between gap-x-4 gap-y-2">
        <h2 id="usage-day-h" className="font-display text-base font-semibold text-ink">
          {label} per {byWeek ? "week" : "day"}
        </h2>
        <Segmented<Measure> label="Measure" value={measure} options={MEASURES} onChange={setMeasure} />
      </div>
      {byWeek ? <p className="text-xs text-ink-2">Shown by week</p> : null}
      {table ? (
        <div className="space-y-2">
          <DataTable caption={`${label} per ${byWeek ? "week" : "day"}`}>
            <thead>
              <tr>
                <Th>{byWeek ? "Week of" : "Day"}</Th>
                <Th className="text-right">{label}</Th>
              </tr>
            </thead>
            <tbody>
              {points.map((p) => (
                <tr key={p.day}>
                  <Td>{p.day}</Td>
                  <Td className="text-right tabular-nums">{fmt(measureValue(p, measure))}</Td>
                </tr>
              ))}
            </tbody>
          </DataTable>
          <Button variant="ghost" size="sm" onClick={() => setTable(false)}>
            Show as chart
          </Button>
        </div>
      ) : (
        <BarChart
          data={points.map((p) => ({
            key: p.day,
            label: byWeek ? `Week of ${shortDay(p.day)}` : shortDay(p.day),
            value: measureValue(p, measure),
          }))}
          format={fmt}
          unitLabel={`${label} per ${byWeek ? "week" : "day"}`}
          onShowTable={() => setTable(true)}
          width={Math.max(160, width)}
        />
      )}
    </section>
  );
}

function Tiles({ t, asOf }: { t: UsageCounters; asOf: string | null | undefined }) {
  const tokens = t.input_tokens + t.output_tokens;
  return (
    <div className="grid grid-cols-2 gap-3 lg:grid-cols-4">
      <StatTile
        label="Tokens"
        value={compact(tokens)}
        title={exact(tokens)}
        detail={
          <>
            Input {compact(t.input_tokens)} · Output {compact(t.output_tokens)} · Cache read {compact(t.cache_read_tokens)} · Cache write{" "}
            {compact(t.cache_write_tokens)}
          </>
        }
      />
      <StatTile label="Turns" value={compact(t.turns)} title={exact(t.turns)} />
      <StatTile label="Turn time" value={duration(t.turn_seconds)} title={`${exact(t.turn_seconds)} s`} />
      <StatTile
        label="Awake time"
        value={duration(t.awake_seconds)}
        title={`${exact(t.awake_seconds)} s`}
        detail={asOf ? `As of ${formatExact(asOf)}` : "Not counted yet"}
      />
    </div>
  );
}

const nameOr = (name: string | null | undefined, fallback: string) =>
  name ? <span>{name}</span> : <span className="text-muted">{fallback}</span>;

/**
 * (phase 5, P5-USAGE-2, D-359, D-360, C-1000, C-1003, C-1004, C-1006; `14 §Usage`) What agents used: tokens, turns,
 * turn time and awake time, in a workspace (Activity › Usage, also filtered to one agent), an organization (Data ›
 * Usage) and the instance (Administration › Data › Usage). One view; the level decides the data, the filter and the
 * tables. No cost anywhere.
 */
export function UsageView({ level }: { level: UsageLevel }) {
  const [params, setParams] = useUsageParams(FILTER_PARAM[level.kind]);
  const { range, filter } = params;
  const preset = range === "custom" ? null : presetRange(RANGE_DAYS[range]);
  const from = preset?.from ?? params.from;
  const to = preset?.to ?? params.to;
  const ready = DATE_RE.test(from) && DATE_RE.test(to);

  const ws = useWorkspaceUsage(level.kind === "workspace" ? level.wsId : undefined, from, to, filter, ready && level.kind === "workspace");
  const org = useOrgUsage(level.kind === "org" ? level.orgId : undefined, from, to, filter, ready && level.kind === "org");
  const inst = useInstanceUsage(from, to, filter, ready && level.kind === "admin");
  const q = level.kind === "workspace" ? ws : level.kind === "org" ? org : inst;

  const agents = useAgents(level.kind === "workspace" ? level.wsId : undefined);
  const workspaces = useWorkspaces(level.kind === "org" ? level.orgId : undefined);
  const orgs = useAdminOrgs(false);

  // a 422 (too long, from after to) belongs to the range fields, not to a page-level error
  const rangeError = q.error instanceof ApiError && q.error.status === 422 ? errorMessage(q.error) : null;

  const picker =
    level.kind === "workspace" ? (
      filter ? (
        <FilterChip
          label={`Agent: ${agents.data?.items.find((a) => a.id === filter)?.name ?? "Deleted agent"}`}
          onRemove={() => setParams({ filter: "" })}
        />
      ) : (
        <Field id="usage-agent" label="Agent">
          <Select id="usage-agent" value={filter} className="w-48 max-w-full" placeholder="All agents" onChange={(v) => setParams({ filter: v })} options={agents.data?.items.map((a) => ({ value: a.id, label: a.name }))} />
        </Field>
      )
    ) : level.kind === "org" ? (
      <Field id="usage-workspace" label="Workspace">
        <Select id="usage-workspace" value={filter} className="w-48 max-w-full" placeholder="All workspaces" onChange={(v) => setParams({ filter: v })} options={workspaces.data?.items.map((w) => ({ value: w.id, label: w.name }))} />
      </Field>
    ) : (
      <Field id="usage-org" label="Organization">
        <Select id="usage-org" value={filter} className="w-48 max-w-full" placeholder="All organizations" onChange={(v) => setParams({ filter: v })} options={orgs.data?.items.map((o) => ({ value: o.id, label: o.name }))} />
      </Field>
    );

  const agentName = level.kind === "workspace" && filter ? (agents.data?.items.find((a) => a.id === filter)?.name ?? "Deleted agent") : null;

  const controls = (
    <div className="flex flex-wrap items-start gap-x-4 gap-y-3 rounded-card border border-line bg-surface p-4 shadow-card">
      <div className="space-y-1.5">
        <span className="block text-sm font-medium text-ink">Range</span>
        <Segmented<RangeId>
          label="Range"
          value={range}
          options={[
            { value: "7d", label: "7 days" },
            { value: "30d", label: "30 days" },
            { value: "90d", label: "90 days" },
            { value: "custom", label: "Custom" },
          ]}
          onChange={(r) => {
            if (r === "custom") {
              const p = presetRange(range === "custom" ? 30 : RANGE_DAYS[range]);
              setParams({ range: "custom", from: params.from || p.from, to: params.to || p.to });
            } else setParams({ range: r, from: "", to: "" });
          }}
        />
      </div>
      {range === "custom" ? (
        <>
          <Field id="usage-from" label="From (UTC)" error={rangeError ?? undefined}>
            <DateField id="usage-from" value={params.from} onChange={(v) => setParams({ from: v })} />
          </Field>
          <Field id="usage-to" label="To (UTC)">
            <DateField id="usage-to" value={params.to} onChange={(v) => setParams({ to: v })} />
          </Field>
        </>
      ) : null}
      {picker}
    </div>
  );

  let body: ReactNode;
  if (!ready) body = null;
  else if (q.isPending) body = <Loading />;
  else if (q.isError) body = rangeError ? null : <ErrorState error={q.error} onRetry={() => void q.refetch()} />;
  else {
    const d = q.data;
    if (isZero(d.totals)) {
      body = <EmptyState title="No usage in these days.">Pick a longer range, or wait for agents to take turns.</EmptyState>;
    } else {
      const days = d.by_day as (DayPoint & UsageCounters)[];
      const tables: ReactNode[] = [];
      if (level.kind === "workspace" && !filter) {
        const wd = d as WorkspaceUsage;
        tables.push(
          <section key="agent" aria-labelledby="usage-agent-h" className="space-y-3">
            <h2 id="usage-agent-h" className="font-display text-base font-semibold text-ink">
              By agent
            </h2>
            <CountersTable
              caption="Usage by agent"
              first="Agent"
              rows={wd.by_agent.map((a) => ({
                key: a.agent_id,
                name:
                  a.agent_name && level.kind === "workspace" ? (
                    <Link href={`${level.base}/agents/${a.agent_id}`} className="text-accent-ink underline-offset-2 hover:underline">
                      {a.agent_name}
                    </Link>
                  ) : (
                    nameOr(a.agent_name, "Deleted agent")
                  ),
                counters: a,
              }))}
            />
          </section>,
        );
      }
      if (level.kind === "admin" && !filter) {
        const idt = d as InstanceUsage;
        tables.push(
          <section key="org" aria-labelledby="usage-org-h" className="space-y-3">
            <h2 id="usage-org-h" className="font-display text-base font-semibold text-ink">
              By organization
            </h2>
            <CountersTable
              caption="Usage by organization"
              first="Organization"
              rows={idt.by_org.map((o) => ({
                key: o.org_id,
                name: (
                  <button
                    type="button"
                    className="text-left text-accent-ink underline-offset-2 hover:underline"
                    onClick={() => setParams({ filter: o.org_id })}
                  >
                    {o.org_name ?? "Deleted organization"}
                  </button>
                ),
                counters: o,
              }))}
            />
          </section>,
        );
      }
      if (level.kind !== "workspace") {
        const wl = d as OrgUsage | InstanceUsage;
        const wsRows: Row[] = wl.by_workspace.map((w) => {
          const label = w.workspace_name ?? "Deleted workspace";
          const slug = level.kind === "org" ? workspaces.data?.items.find((x) => x.id === w.workspace_id)?.slug : undefined;
          return {
            key: w.workspace_id,
            name:
              level.kind === "org" && w.workspace_name && slug ? (
                <Link href={`/o/${level.orgSlug}/w/${slug}/activity?tab=usage`} className="text-accent-ink underline-offset-2 hover:underline">
                  {label}
                </Link>
              ) : (
                nameOr(w.workspace_name, "Deleted workspace")
              ),
            counters: w,
          };
        });
        if (wl.other_workspaces) wsRows.push({ key: "other", name: <span className="text-ink-2">Other workspaces</span>, counters: wl.other_workspaces });
        tables.push(
          <section key="ws" aria-labelledby="usage-ws-h" className="space-y-3">
            <h2 id="usage-ws-h" className="font-display text-base font-semibold text-ink">
              Top workspaces
            </h2>
            <CountersTable caption="Usage by workspace" first="Workspace" rows={wsRows} />
          </section>,
        );
      }
      tables.push(
        <section key="model" aria-labelledby="usage-model-h" className="space-y-3">
          <h2 id="usage-model-h" className="font-display text-base font-semibold text-ink">
            By model
          </h2>
          <ModelsTable models={d.by_model} />
        </section>,
      );
      body = (
        <>
          <Tiles t={d.totals} asOf={d.runtime_as_of} />
          <PerDay days={days} />
          {tables}
        </>
      );
    }
  }

  return (
    <div className="space-y-5" data-testid="usage-view">
      {agentName ? (
        <h2 className="font-display text-lg font-semibold text-ink">Usage of {agentName}</h2>
      ) : null}
      <p className="text-sm text-ink-2">{OPENING}</p>
      {controls}
      {body}
    </div>
  );
}
