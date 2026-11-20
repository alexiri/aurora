"use client";

import { useParams, useSearchParams } from "next/navigation";
import { useState } from "react";

import { fmtDate } from "@/components/account/key-table";
import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { DataTable, Td, Th } from "@/components/ui/data-table";
import { LoadMore } from "@/components/ui/load-more";
import { MoreNotShown } from "@/components/ui/more-not-shown";
import { PageHeader } from "@/components/ui/page-header";
import { Select } from "@/components/ui/select";
import { StatusDot } from "@/components/ui/status-dot";
import { Tabs, tabId, tabPanelId } from "@/components/ui/tabs";
import {
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
import { cn } from "@/lib/utils";

import { useAgents } from "@/lib/api/queries";
import { ApprovalExample } from "./approval-example";
import { AlwaysAllowedNote, ApprovalQuestion } from "./approval-question";
import { OwnSendApprovals } from "./own-send-approvals";
import { ParkedChip, ResumeBadge } from "./parked";

/** Summary text is untrusted data: React text nodes only, never HTML. */
export function ApprovalCard({
  approval: a,
  now,
  onDecide,
  pending,
  message,
  focused = false,
}: {
  approval: Approval;
  now: number;
  onDecide: (d: Decision, always?: boolean) => void;
  pending: boolean;
  message?: string;
  /** `?focus=<id>` (a notification's link): scrolled into view and outlined. */
  focused?: boolean;
}) {
  const params = useParams<{ org?: string; ws?: string }>();
  // A parked approval repeats its sentence before it is decided (inside the card): the agent is told, not resumed in place.
  return (
    // (P5-REFINE-19) `a` / `b` answer the card that has the focus (Expert): the same buttons.
    <li tabIndex={-1} data-approval-card="" className={cn("rounded-card border border-pending-line bg-surface p-4 outline-none shadow-card focus-visible:ring-2 focus-visible:ring-accent sm:p-5", focused && "ring-2 ring-accent")}>
      <ApprovalQuestion
        approval={a}
        now={now}
        onDecide={onDecide}
        pending={pending}
        message={message}
        focused={focused}
        canApprove
        org={params?.org ?? ""}
        ws={params?.ws ?? ""}
      />
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

/** "3 requests are waiting for you." / "1 request is waiting for you." / "Nothing is waiting for you." */
export function approvalsSentence(n: number): string {
  if (n === 0) return "Nothing is waiting for you.";
  return n === 1 ? "1 request is waiting for you." : `${n} requests are waiting for you.`;
}

/** The gateway's approvals and the action approvals as one list, newest first. */
export function mergePending(
  gateway: Approval[],
  actions: ActionApproval[],
): ({ kind: "gateway"; item: Approval } | { kind: "action"; item: ActionApproval })[] {
  return [
    ...gateway.map((item) => ({ kind: "gateway" as const, item })),
    ...actions.map((item) => ({ kind: "action" as const, item })),
  ].sort((x, y) => Date.parse(y.item.created_at) - Date.parse(x.item.created_at));
}

/** The workspace's one agent by name, or "your agents" (workspace-11). */
function useAgentLabel(wsId: string): { name: string; who: string; many: boolean } {
  const agents = useAgents(wsId).data?.items ?? [];
  const first = agents[0]?.name ?? "Your agent";
  return agents.length === 1 ? { name: first, who: first, many: false } : { name: first, who: "one of your agents", many: true };
}

function PendingTab({ wsId }: { wsId: string }) {
  const q = usePendingApprovals(wsId);
  const { live } = useApprovalsStream(wsId);
  const now = useNow();
  const decide = useDecide(wsId);
  const batch = useDecideBatch(wsId);
  const actions = usePendingActionApprovals(wsId);
  const decideAction = useDecideAction(wsId);
  const actionItems = actions.data?.items ?? [];
  const [messages, setMessages] = useState<Record<string, string>>({});
  const [batchError, setBatchError] = useState<string | null>(null);
  function onDecideAction(id: string, decision: Decision) {
    setMessages((m) => ({ ...m, [id]: "" }));
    decideAction.mutate(
      { id, decision },
      { onError: (err) => setMessages((m) => ({ ...m, [id]: actionAlreadyDecided(err) ? "Already decided by someone else." : errorMessage(err) })) },
    );
  }
  // A notification's link (in-tab or push) opens `…/approvals?focus=<id>`.
  const focus = useSearchParams()?.get("focus") ?? null;

  const params = useParams<{ org?: string; ws?: string }>();
  const agentLabel = useAgentLabel(wsId);
  // What Always allow just did (C-982): "Daneel will no longer ask before …", kept until the next answer.
  const [always, setAlways] = useState<{ approval: Approval; also: number } | null>(null);

  function onDecide(a: Approval, decision: Decision, wantsAlways = false) {
    const id = a.id;
    setMessages((m) => ({ ...m, [id]: "" }));
    decide.mutate(
      { id, decision, always: wantsAlways },
      {
        onSuccess: (done) => setAlways(wantsAlways && done.always_applied ? { approval: a, also: done.always_applied.also_approved } : null),
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
  // `?focus=<id>` puts that question first (C-983).
  const items = focus ? [...q.data.items].sort((x, y) => Number(y.id === focus) - Number(x.id === focus)) : q.data.items;
  const groups = groupApprovals(items);
  // (P5-POLISH-7, D3) One list: the requests of the gateway and the action approvals together, newest first. Batches stay grouped.
  const batched = groups.filter((g) => g.batchId && g.agentId);
  const loose = groups.filter((g) => !(g.batchId && g.agentId)).flatMap((g) => g.items);
  const merged = mergePending(loose, actionItems);
  const total = items.length + actionItems.length;
  return (
    <div className="space-y-4">
      <p role="status" className="flex flex-wrap items-center gap-x-4 gap-y-1 text-sm text-ink-2">
        {/* (workspace-11) "Nothing is waiting for you." is the empty state's title below, so it is not said twice. */}
        {total > 0 ? <span data-testid="approvals-sentence">{approvalsSentence(total)}</span> : null}
        <StatusDot tone={live ? "accent" : "pending"}>{live ? "Live" : "Reconnecting…"}</StatusDot>
      </p>
      {batchError ? (
        <p role="alert" className="text-sm text-danger-ink">
          {batchError}
        </p>
      ) : null}
      {always ? (
        <section aria-label="Answered just now" className="rounded-card border border-dashed border-line-strong p-4">
          <p className="mb-1 text-sm text-ink">
            You chose <strong>Always allow</strong> for {always.approval.sentence.agent.name}.
          </p>
          <AlwaysAllowedNote
            agentName={always.approval.sentence.agent.name}
            agentHandle={always.approval.agent?.handle}
            action={always.approval.sentence.action}
            appName={always.approval.sentence.app?.name}
            alsoApproved={always.also}
            org={params?.org ?? ""}
            ws={params?.ws ?? ""}
          />
        </section>
      ) : null}
      {q.data.truncated ? <MoreNotShown what="pending approvals" narrow="Decide the ones shown; the rest appear as these are decided." /> : null}
      {total === 0 ? (
        <>
          <EmptyState title="Nothing is waiting for you.">
            When {agentLabel.who} needs a yes from you, it shows up here and in your notices.
          </EmptyState>
          {/* Only while nothing is waiting: a real question and the sample are never on screen together. */}
          <ApprovalExample agentName={agentLabel.name} org={params?.org ?? ""} ws={params?.ws ?? ""} />
        </>
      ) : null}
      {merged.length > 0 ? (
        <ul className="space-y-2" aria-label="Waiting for you">
          {merged.map((m) =>
            m.kind === "gateway" ? (
              <ApprovalCard
                key={m.item.id}
                approval={m.item}
                now={now}
                pending={decide.isPending && decide.variables?.id === m.item.id}
                message={messages[m.item.id]}
                focused={focus === m.item.id}
                onDecide={(d, al) => onDecide(m.item, d, al === true)}
              />
            ) : (
              <ActionApprovalCard
                key={m.item.id}
                a={m.item}
                pending={decideAction.isPending && decideAction.variables?.id === m.item.id}
                message={messages[m.item.id]}
                onDecide={(d) => onDecideAction(m.item.id, d)}
              />
            ),
          )}
        </ul>
      ) : null}
      {batched.map((g) => (
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
                  {d === "approve" ? `Allow all ${g.items.length} once` : "Block all"}
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
                focused={focus === a.id}
                onDecide={(d, al) => onDecide(a, d, al === true)}
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
      {a.action_key === "send_as_me" ? (
        // (P4-UTOK, D-283) A post as a person is decided by that person alone, on their own page with the full text.
        <p className="text-sm text-ink-2">Only the person it would speak for can decide this.</p>
      ) : (
        <div className="flex flex-wrap justify-end gap-2">
          <Button size="lg" variant="outline" disabled={pending} onClick={() => onDecide("deny")}>
            Block
          </Button>
          <Button size="lg" variant="approve" disabled={pending} onClick={() => onDecide("approve")}>
            Allow once
          </Button>
        </div>
      )}
    </li>
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
        <Select value={status} onChange={setStatus} options={ACTION_STATUSES.map((s) => ({ value: s, label: s || "All" }))} />
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
                  <Badge tone={a.status === "failed" || a.status === "rejected" ? "danger" : a.status === "expired" ? "neutral" : "accent"}>
                    {a.status}
                  </Badge>
                </Td>
                <Td>{a.decided_by?.display_name ?? "—"}</Td>
              </tr>
            ))}
          </tbody>
        </DataTable>
      ) : null}
      <LoadMore query={q} />
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
        <Select value={status} onChange={setStatus} options={STATUSES.map((s) => ({ value: s, label: s || "All" }))} />
      </label>
      {q.isPending ? <Loading /> : null}
      {q.isError ? <ErrorState error={q.error} onRetry={() => q.refetch()} /> : null}
      {q.data && items.length === 0 ? <EmptyState title="No questions have been answered yet." /> : null}
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
                <Td>
                  {a.sentence.agent.name} wants to {a.sentence.action}
                  {a.sentence.app ? ` in ${a.sentence.app.name}` : ""}
                </Td>
                <Td>
                  <Badge tone={a.status === "approved" ? "accent" : a.status === "denied" ? "danger" : "neutral"}>{a.status}</Badge>
                  {a.parked ? (
                    <span className="mt-1 flex flex-wrap items-center gap-1">
                      <ParkedChip />
                      <ResumeBadge approval={a} />
                    </span>
                  ) : null}
                </Td>
                <Td>{a.decided_by?.display_name ?? "—"}</Td>
              </tr>
            ))}
          </tbody>
        </DataTable>
      ) : null}
      <LoadMore query={q} />
    </div>
  );
}

export function ApprovalsView({ wsId }: { wsId: string }) {
  const [tab, setTab] = useState<"pending" | "history">("pending");
  const agent = useAgentLabel(wsId);
  return (
    <div className="max-w-5xl">
      <PageHeader title="Approvals" description={agent.many ? "Your agents ask you here before they change something." : `${agent.name} asks you here before it changes something.`} />
      <Tabs
        items={[
          { value: "pending", label: "Waiting" },
          { value: "history", label: "History" },
        ]}
        value={tab}
        onValueChange={(v) => setTab(v as "pending" | "history")}
        label="Approvals"
        idBase="approvals"
        className="mb-4"
      />
      <div role="tabpanel" id={tabPanelId("approvals", tab)} aria-labelledby={tabId("approvals", tab)} className="space-y-8">
        {tab === "pending" ? <OwnSendApprovals /> : null}
        {tab === "pending" ? <PendingTab wsId={wsId} /> : <HistoryTab wsId={wsId} />}
        {/* The waiting action approvals are in the one list above; their history keeps its own section. */}
        {tab === "history" ? (
          <section aria-label="Action approvals" className="space-y-3">
            <h2 className="text-lg font-semibold text-ink">Action approvals</h2>
            <ActionHistory wsId={wsId} />
          </section>
        ) : null}
      </div>
    </div>
  );
}
