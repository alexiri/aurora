"use client";

import { useState } from "react";

import { fmtDate } from "@/components/account/key-table";
import { Avatar } from "@/components/agent-avatar";
import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { DataTable, Td, Th } from "@/components/ui/data-table";
import { inputClass } from "@/components/ui/field";
import { PageHeader } from "@/components/ui/page-header";
import { StatusDot } from "@/components/ui/status-dot";
import { Tabs, tabId, tabPanelId } from "@/components/ui/tabs";
import {
  countdown,
  isAlreadyDecided,
  useApprovalHistory,
  useApprovalsStream,
  useDecide,
  useDecideBatch,
  useNow,
  usePendingApprovals,
  type Approval,
  type Decision,
} from "@/lib/api/approvals";
import {
  isAlreadyDecided as actionAlreadyDecided,
  useActionApprovalHistory,
  useDecideAction,
  usePendingActionApprovals,
  type ActionApproval,
} from "@/lib/api/action-approvals";
import { errorMessage } from "@/lib/api/errors";

/** Summary text is untrusted data: React text nodes only, never HTML. */
export function ApprovalCard({
  approval: a,
  now,
  onDecide,
  pending,
  message,
}: {
  approval: Approval;
  now: number;
  onDecide: (d: Decision) => void;
  pending: boolean;
  message?: string;
}) {
  return (
    <li className="space-y-3 rounded-card border border-line bg-surface p-4 text-ink shadow-card sm:p-5">
      <div className="flex flex-wrap items-center gap-x-2 gap-y-1 text-sm">
        {a.agent ? <Avatar name={a.agent.name} tintKey={a.agent.id} size={24} /> : null}
        <span className="font-semibold">{a.agent ? a.agent.name : "Unknown agent"}</span>
        {a.agent ? <span className="font-mono text-muted">@{a.agent.handle}</span> : null}
        <span className="ml-auto font-medium tabular-nums text-pending-ink">Expires in {countdown(a.deadline, now)}</span>
      </div>
      <p className="font-display text-base font-semibold">{a.summary.title}</p>
      {a.summary.text ? <p className="whitespace-pre-wrap text-sm text-ink-2">{a.summary.text}</p> : null}
      <p className="flex min-w-0 flex-wrap items-center gap-2 rounded-control bg-canvas px-3 py-2 font-mono text-xs text-ink-2">
        <span className="rounded bg-subtle px-1.5 py-0.5 font-semibold text-ink">{a.method}</span>
        <span className="min-w-0 break-all">
          {a.host}
          {a.path}
        </span>
      </p>
      {a.summary.fields.length > 0 ? (
        <dl className="grid grid-cols-[max-content_1fr] gap-x-4 gap-y-1 text-sm">
          {a.summary.fields.map((f, i) => (
            <div key={i} className="contents">
              <dt className="text-muted">{f.label}</dt>
              <dd className="break-words">{f.value}</dd>
            </div>
          ))}
        </dl>
      ) : null}
      {a.rule_name ? <p className="text-xs text-muted">Rule: {a.rule_name}</p> : null}
      {a.context_label ? (
        <p className="text-xs text-muted" data-testid="context-label">
          Run: {a.context_label}
        </p>
      ) : null}
      {message ? (
        <p role="alert" className="rounded-control border border-pending-line bg-pending-panel p-3 text-sm text-pending-ink">
          {message}
        </p>
      ) : null}
      <div className="flex flex-wrap justify-end gap-2">
        <Button size="lg" variant="outline" disabled={pending} onClick={() => onDecide("deny")}>
          Deny
        </Button>
        <Button size="lg" variant="approve" disabled={pending} onClick={() => onDecide("approve")}>
          Approve
        </Button>
      </div>
    </li>
  );
}

interface Group {
  key: string;
  label: string | null;
  batchId: string | null;
  agentId: string | null;
  items: Approval[];
}

export function groupApprovals(items: Approval[]): Group[] {
  const groups: Group[] = [];
  const byBatch = new Map<string, Group>();
  for (const a of items) {
    const b = a.batch;
    if (!b || !a.agent) {
      groups.push({ key: a.id, label: null, batchId: null, agentId: null, items: [a] });
      continue;
    }
    const k = `${b.id}:${a.agent.id}`;
    let g = byBatch.get(k);
    if (!g) {
      g = { key: k, label: b.label ?? null, batchId: b.id, agentId: a.agent.id, items: [] };
      byBatch.set(k, g);
      groups.push(g);
    }
    g.items.push(a);
  }
  return groups;
}

function PendingTab({ wsId }: { wsId: string }) {
  const q = usePendingApprovals(wsId);
  const { live } = useApprovalsStream(wsId);
  const now = useNow();
  const decide = useDecide(wsId);
  const batch = useDecideBatch(wsId);
  const [messages, setMessages] = useState<Record<string, string>>({});
  const [batchError, setBatchError] = useState<string | null>(null);

  function onDecide(id: string, decision: Decision) {
    setMessages((m) => ({ ...m, [id]: "" }));
    decide.mutate(
      { id, decision },
      {
        onError: (err) =>
          setMessages((m) => ({
            ...m,
            [id]: isAlreadyDecided(err) ? "Already decided by someone else." : errorMessage(err),
          })),
      },
    );
  }

  if (q.isPending) return <Loading />;
  if (q.isError) return <ErrorState error={q.error} onRetry={() => q.refetch()} />;
  const groups = groupApprovals(q.data.items);
  return (
    <div className="space-y-4">
      <p role="status">
        <StatusDot tone={live ? "accent" : "pending"}>{live ? "Live" : "Reconnecting…"}</StatusDot>
      </p>
      {batchError ? (
        <p role="alert" className="text-sm text-danger-ink">
          {batchError}
        </p>
      ) : null}
      {groups.length === 0 ? (
        <EmptyState title="No pending approvals" when="Look here when a number appears next to Approvals in the menu.">
          Nothing is waiting for you. When an agent wants to do something risky, it asks here first.
        </EmptyState>
      ) : null}
      {groups.map((g) => (
        <section key={g.key} aria-label={g.label ?? undefined} className="space-y-2">
          {g.batchId && g.agentId ? (
            <div className="flex flex-wrap items-center gap-2 rounded-control bg-subtle px-3 py-2">
              <h2 className="mr-auto text-sm font-semibold text-ink">{g.label ?? "Batch"}</h2>
              {(["approve", "deny"] as const).map((d) => (
                <Button
                  key={d}
                  size="sm"
                  variant={d === "approve" ? "approve" : "outline"}
                  disabled={batch.isPending}
                  onClick={() => {
                    setBatchError(null);
                    batch.mutate(
                      { batchId: g.batchId!, agentId: g.agentId!, decision: d },
                      { onError: (err) => setBatchError(errorMessage(err)) },
                    );
                  }}
                >
                  {d === "approve" ? `Approve all ${g.items.length}` : "Deny all"}
                </Button>
              ))}
            </div>
          ) : null}
          <ul className="space-y-2">
            {g.items.map((a) => (
              <ApprovalCard
                key={a.id}
                approval={a}
                now={now}
                pending={decide.isPending && decide.variables?.id === a.id}
                message={messages[a.id]}
                onDecide={(d) => onDecide(a.id, d)}
              />
            ))}
          </ul>
        </section>
      ))}
    </div>
  );
}

/** `summary` is platform-composed text; it is rendered as a plain React text node only. */
export function ActionApprovalCard({
  a,
  pending,
  message,
  onDecide,
}: {
  a: ActionApproval;
  pending: boolean;
  message?: string;
  onDecide: (d: Decision) => void;
}) {
  return (
    <li className="space-y-3 rounded-card border border-line bg-surface p-4 text-ink shadow-card sm:p-5">
      <p className="whitespace-pre-wrap font-display text-base font-semibold">{a.summary}</p>
      <p className="text-xs text-muted">
        <span className="font-mono">{a.action_key}</span> · expires {fmtDate(a.expires_at)}
      </p>
      {message ? (
        <p role="alert" className="rounded-control border border-pending-line bg-pending-panel p-3 text-sm text-pending-ink">
          {message}
        </p>
      ) : null}
      <div className="flex flex-wrap justify-end gap-2">
        <Button size="lg" variant="outline" disabled={pending} onClick={() => onDecide("deny")}>
          Deny
        </Button>
        <Button size="lg" variant="approve" disabled={pending} onClick={() => onDecide("approve")}>
          Approve
        </Button>
      </div>
    </li>
  );
}

function ActionPending({ wsId }: { wsId: string }) {
  const q = usePendingActionApprovals(wsId);
  const decide = useDecideAction(wsId);
  const [messages, setMessages] = useState<Record<string, string>>({});
  function onDecide(id: string, decision: Decision) {
    setMessages((m) => ({ ...m, [id]: "" }));
    decide.mutate(
      { id, decision },
      {
        onError: (err) =>
          setMessages((m) => ({ ...m, [id]: actionAlreadyDecided(err) ? "Already decided by someone else." : errorMessage(err) })),
      },
    );
  }
  if (q.isPending) return <Loading />;
  if (q.isError) return <ErrorState error={q.error} onRetry={() => void q.refetch()} />;
  if (q.data.items.length === 0) return <EmptyState title="No pending action approvals" />;
  return (
    <ul className="space-y-2">
      {q.data.items.map((a) => (
        <ActionApprovalCard
          key={a.id}
          a={a}
          pending={decide.isPending && decide.variables?.id === a.id}
          message={messages[a.id]}
          onDecide={(d) => onDecide(a.id, d)}
        />
      ))}
    </ul>
  );
}

const ACTION_STATUSES = ["", "approved", "executed", "failed", "rejected", "expired"];

function ActionHistory({ wsId }: { wsId: string }) {
  const [status, setStatus] = useState("");
  const q = useActionApprovalHistory(wsId, status);
  const items = (q.data?.pages.flatMap((p) => p.items) ?? []).filter((a) => a.status !== "pending");
  return (
    <div className="space-y-3">
      <label className="flex max-w-xs items-center gap-2 text-sm">
        Action status
        <select className={inputClass} value={status} onChange={(e) => setStatus(e.target.value)}>
          {ACTION_STATUSES.map((s) => (
            <option key={s} value={s}>
              {s || "All"}
            </option>
          ))}
        </select>
      </label>
      {q.isPending ? <Loading /> : null}
      {q.isError ? <ErrorState error={q.error} onRetry={() => void q.refetch()} /> : null}
      {q.data && items.length === 0 ? <EmptyState title="No action approvals yet" /> : null}
      {items.length > 0 ? (
        <DataTable caption="Action approval history">
          <thead>
            <tr>
              <Th>When</Th>
              <Th>Summary</Th>
              <Th>Status</Th>
              <Th>Decided by</Th>
            </tr>
          </thead>
          <tbody>
            {items.map((a) => (
              <tr key={a.id}>
                <Td>{fmtDate(a.decided_at ?? a.created_at)}</Td>
                <Td>
                  {a.summary}
                  {a.error ? <span className="block text-xs text-danger-ink">{a.error}</span> : null}
                </Td>
                <Td>
                  <Badge tone={a.status === "failed" || a.status === "rejected" ? "danger" : a.status === "expired" ? "pending" : "accent"}>
                    {a.status}
                  </Badge>
                </Td>
                <Td>{a.decided_by?.display_name ?? "—"}</Td>
              </tr>
            ))}
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

const STATUSES = ["", "approved", "denied", "expired", "abandoned"];

function HistoryTab({ wsId }: { wsId: string }) {
  const [status, setStatus] = useState("");
  const q = useApprovalHistory(wsId, status);
  const items = (q.data?.pages.flatMap((p) => p.items) ?? []).filter((a) => a.status !== "pending");
  return (
    <div className="space-y-4">
      <label className="flex max-w-xs items-center gap-2 text-sm">
        Status
        <select className={inputClass} value={status} onChange={(e) => setStatus(e.target.value)}>
          {STATUSES.map((s) => (
            <option key={s} value={s}>
              {s || "All"}
            </option>
          ))}
        </select>
      </label>
      {q.isPending ? <Loading /> : null}
      {q.isError ? <ErrorState error={q.error} onRetry={() => q.refetch()} /> : null}
      {q.data && items.length === 0 ? <EmptyState title="No approvals yet" /> : null}
      {items.length > 0 ? (
        <DataTable caption="Approval history">
          <thead>
            <tr>
              <Th>When</Th>
              <Th>Agent</Th>
              <Th>Request</Th>
              <Th>Summary</Th>
              <Th>Status</Th>
              <Th>Decided by</Th>
            </tr>
          </thead>
          <tbody>
            {items.map((a) => (
              <tr key={a.id}>
                <Td>{fmtDate(a.decided_at ?? a.created_at)}</Td>
                <Td>{a.agent?.name ?? "—"}</Td>
                <Td className="font-mono">
                  {a.method} {a.host}
                  {a.path}
                </Td>
                <Td>{a.summary.title}</Td>
                <Td>
                  <Badge tone={a.status === "approved" ? "accent" : a.status === "denied" ? "danger" : "neutral"}>{a.status}</Badge>
                </Td>
                <Td>{a.decided_by?.display_name ?? "—"}</Td>
              </tr>
            ))}
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

export function ApprovalsView({ wsId }: { wsId: string }) {
  const [tab, setTab] = useState<"pending" | "history">("pending");
  return (
    <div className="max-w-5xl">
      <PageHeader title="Approvals" description="Requests that wait for a person to allow or deny them." />
      <Tabs
        items={[
          { value: "pending", label: "Pending" },
          { value: "history", label: "History" },
        ]}
        value={tab}
        onValueChange={(v) => setTab(v as "pending" | "history")}
        label="Approvals"
        idBase="approvals"
        className="mb-4"
      />
      <div role="tabpanel" id={tabPanelId("approvals", tab)} aria-labelledby={tabId("approvals", tab)} className="space-y-8">
        {tab === "pending" ? <PendingTab wsId={wsId} /> : <HistoryTab wsId={wsId} />}
        <section aria-label="Action approvals" className="space-y-3">
          <h2 className="text-lg font-semibold text-ink">Action approvals</h2>
          {tab === "pending" ? <ActionPending wsId={wsId} /> : <ActionHistory wsId={wsId} />}
        </section>
      </div>
    </div>
  );
}
