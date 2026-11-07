"use client";

import { useState } from "react";

import { fmtDate } from "@/components/account/key-table";
import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import { inputClass } from "@/components/ui/field";
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
    <li className="space-y-2 rounded-md border border-neutral-200 p-4">
      <div className="flex flex-wrap items-baseline gap-2 text-sm">
        <span className="font-medium">{a.agent ? a.agent.name : "Unknown agent"}</span>
        {a.agent ? <span className="text-neutral-500">@{a.agent.handle}</span> : null}
        <span className="font-mono text-neutral-700">
          {a.method} {a.host}
          {a.path}
        </span>
        <span className="ml-auto text-neutral-500">Expires in {countdown(a.deadline, now)}</span>
      </div>
      <p className="font-medium">{a.summary.title}</p>
      {a.summary.text ? <p className="whitespace-pre-wrap text-sm">{a.summary.text}</p> : null}
      {a.summary.fields.length > 0 ? (
        <dl className="grid grid-cols-[max-content_1fr] gap-x-4 gap-y-1 text-sm">
          {a.summary.fields.map((f, i) => (
            <div key={i} className="contents">
              <dt className="text-neutral-500">{f.label}</dt>
              <dd className="break-words">{f.value}</dd>
            </div>
          ))}
        </dl>
      ) : null}
      {a.rule_name ? <p className="text-xs text-neutral-500">Rule: {a.rule_name}</p> : null}
      {a.context_label ? (
        <p className="text-xs text-neutral-500" data-testid="context-label">
          Run: {a.context_label}
        </p>
      ) : null}
      {message ? (
        <p role="alert" className="text-sm text-amber-800">
          {message}
        </p>
      ) : null}
      <div className="flex gap-2">
        <Button size="sm" disabled={pending} onClick={() => onDecide("approve")}>
          Approve
        </Button>
        <Button size="sm" variant="outline" disabled={pending} onClick={() => onDecide("deny")}>
          Deny
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
      <p className="text-xs text-neutral-500" role="status">
        {live ? "Live" : "Reconnecting…"}
      </p>
      {batchError ? (
        <p role="alert" className="text-sm text-red-700">
          {batchError}
        </p>
      ) : null}
      {groups.length === 0 ? <EmptyState title="No pending approvals" /> : null}
      {groups.map((g) => (
        <section key={g.key} aria-label={g.label ?? undefined} className="space-y-2">
          {g.batchId && g.agentId ? (
            <div className="flex items-center gap-2">
              <h2 className="text-sm font-medium">{g.label ?? "Batch"}</h2>
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
    <li className="space-y-2 rounded-md border border-neutral-200 p-4">
      <p className="whitespace-pre-wrap font-medium">{a.summary}</p>
      <p className="text-xs text-neutral-500">
        <span className="font-mono">{a.action_key}</span> · expires {fmtDate(a.expires_at)}
      </p>
      {message ? (
        <p role="alert" className="text-sm text-amber-800">
          {message}
        </p>
      ) : null}
      <div className="flex gap-2">
        <Button size="sm" disabled={pending} onClick={() => onDecide("approve")}>
          Approve
        </Button>
        <Button size="sm" variant="outline" disabled={pending} onClick={() => onDecide("deny")}>
          Deny
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
        <table className="w-full text-left text-sm">
          <caption className="sr-only">Action approval history</caption>
          <thead className="text-neutral-500">
            <tr>
              <th className="py-2 pr-4 font-medium">When</th>
              <th className="py-2 pr-4 font-medium">Summary</th>
              <th className="py-2 pr-4 font-medium">Status</th>
              <th className="py-2 font-medium">Decided by</th>
            </tr>
          </thead>
          <tbody>
            {items.map((a) => (
              <tr key={a.id} className="border-t border-neutral-200">
                <td className="py-2 pr-4">{fmtDate(a.decided_at ?? a.created_at)}</td>
                <td className="py-2 pr-4">
                  {a.summary}
                  {a.error ? <span className="block text-xs text-red-700">{a.error}</span> : null}
                </td>
                <td className="py-2 pr-4">{a.status}</td>
                <td className="py-2">{a.decided_by?.display_name ?? "—"}</td>
              </tr>
            ))}
          </tbody>
        </table>
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
        <table className="w-full text-left text-sm">
          <thead className="text-neutral-500">
            <tr>
              <th className="py-2 pr-4 font-medium">When</th>
              <th className="py-2 pr-4 font-medium">Agent</th>
              <th className="py-2 pr-4 font-medium">Request</th>
              <th className="py-2 pr-4 font-medium">Summary</th>
              <th className="py-2 pr-4 font-medium">Status</th>
              <th className="py-2 font-medium">Decided by</th>
            </tr>
          </thead>
          <tbody>
            {items.map((a) => (
              <tr key={a.id} className="border-t border-neutral-200">
                <td className="py-2 pr-4">{fmtDate(a.decided_at ?? a.created_at)}</td>
                <td className="py-2 pr-4">{a.agent?.name ?? "—"}</td>
                <td className="py-2 pr-4 font-mono">
                  {a.method} {a.host}
                  {a.path}
                </td>
                <td className="py-2 pr-4">{a.summary.title}</td>
                <td className="py-2 pr-4">{a.status}</td>
                <td className="py-2">{a.decided_by?.display_name ?? "—"}</td>
              </tr>
            ))}
          </tbody>
        </table>
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
    <div className="max-w-5xl space-y-4">
      <h1 className="text-xl font-semibold">Approvals</h1>
      <div role="tablist" aria-label="Approvals" className="flex gap-2 border-b border-neutral-200">
        {(["pending", "history"] as const).map((t) => (
          <button
            key={t}
            role="tab"
            type="button"
            aria-selected={tab === t}
            onClick={() => setTab(t)}
            className={`px-3 py-1.5 text-sm ${tab === t ? "border-b-2 border-neutral-900 font-medium" : "text-neutral-500"}`}
          >
            {t === "pending" ? "Pending" : "History"}
          </button>
        ))}
      </div>
      <div role="tabpanel" className="space-y-8">
        {tab === "pending" ? <PendingTab wsId={wsId} /> : <HistoryTab wsId={wsId} />}
        <section aria-label="Action approvals" className="space-y-3">
          <h2 className="text-lg font-semibold">Action approvals</h2>
          {tab === "pending" ? <ActionPending wsId={wsId} /> : <ActionHistory wsId={wsId} />}
        </section>
      </div>
    </div>
  );
}
