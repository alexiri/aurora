"use client";

import Link from "next/link";
import { useState, type ReactNode } from "react";

import type { AgentPageId } from "@/components/agents/sections";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { StatusDot } from "@/components/ui/status-dot";
import { useDecideAction } from "@/lib/api/action-approvals";
import { countdown, isAlreadyDecided, useDecide, useNow } from "@/lib/api/approvals";
import { approvalAnchorId } from "@/components/chat/approval-card";
import { formatDuration } from "@/lib/chat/layout";
import { cn } from "@/lib/utils";

import { decisionBadge, sandboxLook, type AgentStatus, type NeedsItem, type OpenTurn } from "./status-data";

/** At most this many pending items are listed; more link to the Approvals page. */
export const NEEDS_LIMIT = 5;


/** Scrolls to the open conversation's inline card for `approvalId` and focuses it. Returns whether it was found. */
export function showApprovalInConversation(approvalId: string): boolean {
  const el = document.getElementById(approvalAnchorId(approvalId));
  if (!el) return false;
  el.scrollIntoView({ block: "center" });
  el.focus();
  return true;
}

/** A part's heading inside the expanded rail and the sheet (the collapsed strip shows only the icon). */
export function PartHeading({ icon, children, id, pending = false }: { icon: ReactNode; children: ReactNode; id: string; pending?: boolean }) {
  return (
    <h3 id={id} className="flex items-center gap-2 text-label font-semibold uppercase tracking-label text-muted">
      {/* Needs you carries its icon on a `pending-soft` tile (the board's Needs-you heading). */}
      <span aria-hidden="true" className={cn("inline-flex", pending && "size-6 items-center justify-center rounded-md bg-pending-soft text-pending-ink")}>
        {icon}
      </span>
      {children}
    </h3>
  );
}

// ---- Needs you -------------------------------------------------------------------------------------------------

function NeedsRow({
  item,
  wsId,
  openConversationId,
  now,
  onShow,
}: {
  item: NeedsItem;
  wsId: string;
  openConversationId: string | null;
  now: number;
  onShow: (approvalId: string) => void;
}) {
  const gatewayDecide = useDecide(wsId);
  const actionDecide = useDecideAction(wsId);
  const decide = item.kind === "gateway" ? gatewayDecide : actionDecide;
  // The inline approval card (gateway holds) already has the buttons: link to it instead of repeating them.
  const inline = item.kind === "gateway" && item.conversationId !== null && item.conversationId === openConversationId;
  const left = countdown(item.deadline, now);
  const send = (decision: "approve" | "deny") => {
    if (item.kind === "gateway") gatewayDecide.mutate({ id: item.id, decision });
    else actionDecide.mutate({ id: item.id, decision });
  };
  return (
    <li className="space-y-1.5 rounded-control border border-pending-line bg-pending-panel p-2.5 text-sm text-ink">
      <div className="flex items-baseline justify-between gap-2">
        <p className="min-w-0 truncate text-xs font-semibold text-pending-ink">{item.title ?? "Conversation"}</p>
        <span className="shrink-0 text-xs tabular-nums text-ink-2">{left === "expired" ? "Expired" : `${left} left`}</span>
      </div>
      <p className="break-words">{item.summary}</p>
      <p className="break-all font-mono text-xs text-ink-2">{item.requestLine}</p>
      {inline ? (
        <Button type="button" variant="outline" size="sm" onClick={() => onShow(item.id)}>
          Show in conversation
        </Button>
      ) : (
        <div className="flex gap-2">
          <Button type="button" variant="approve" size="sm" disabled={decide.isPending} onClick={() => send("approve")}>
            Approve
          </Button>
          <Button type="button" variant="outline" size="sm" disabled={decide.isPending} onClick={() => send("deny")}>
            Deny
          </Button>
        </div>
      )}
      {decide.error ? (
        <p role="alert" className="text-xs text-danger-ink">
          {isAlreadyDecided(decide.error) ? "Already decided." : "Could not record the decision."}
        </p>
      ) : null}
    </li>
  );
}

/** The pending items of this agent, soonest first (also the pop-over's and the sheet's body). */
export function NeedsYouList({
  needs,
  loading,
  wsId,
  base,
  openConversationId,
  onShow,
}: {
  needs: NeedsItem[];
  loading: boolean;
  wsId: string;
  base: string;
  openConversationId: string | null;
  onShow: (approvalId: string) => void;
}) {
  const now = useNow(1000);
  if (loading) return <p className="text-sm text-muted">Loading…</p>;
  if (needs.length === 0) return <p className="text-sm text-ink-2">Nothing is waiting for you.</p>;
  const shown = needs.slice(0, NEEDS_LIMIT);
  const more = needs.length - shown.length;
  return (
    <div className="space-y-2">
      <ul className="space-y-2">
        {shown.map((n) => (
          <NeedsRow key={n.key} item={n} wsId={wsId} openConversationId={openConversationId} now={now} onShow={onShow} />
        ))}
      </ul>
      {more > 0 ? (
        <Link href={`${base}/approvals`} className="block text-sm text-link underline">
          {more} more on Approvals
        </Link>
      ) : null}
    </div>
  );
}

// ---- Doing now -------------------------------------------------------------------------------------------------

export function DoingNow({
  working,
  open,
  openConversation,
  titleOf,
  onSelect,
}: {
  working: AgentStatus["working"];
  open: OpenTurn;
  openConversation: { id: string; title: string } | null;
  titleOf: (c: AgentStatus["working"][number]) => string;
  onSelect: (id: string) => void;
}) {
  const rows = working.map((c) => ({
    id: c.id,
    title: titleOf(c),
    // The open conversation's running tool comes from its stream.
    text: openConversation?.id === c.id && open.line ? open.line : "working",
  }));
  // The open conversation can be mid-turn before the list has caught up.
  if (openConversation && open.active && !rows.some((r) => r.id === openConversation.id)) {
    rows.unshift({ id: openConversation.id, title: openConversation.title, text: open.line ?? "working" });
  }
  if (rows.length === 0) return <p className="text-sm text-ink-2">Idle</p>;
  return (
    <ul className="space-y-1">
      {rows.map((r) => (
        <li key={r.id}>
          <button
            type="button"
            onClick={() => onSelect(r.id)}
            className="flex min-h-9 w-full min-w-0 items-center gap-2 rounded-control px-2 py-1 text-left text-sm text-ink hover:bg-subtle"
          >
            <StatusDot tone="accent" className="shrink-0">
              <span className="sr-only">Working</span>
            </StatusDot>
            <span className="min-w-0 truncate">
              {r.title} · {r.text}
            </span>
          </button>
        </li>
      ))}
    </ul>
  );
}

// ---- Recent tool calls -----------------------------------------------------------------------------------------

export function RecentCalls({ status, activityHref }: { status: AgentStatus; activityHref: string }) {
  return (
    <div className="space-y-2">
      {status.recentLoading ? <p className="text-sm text-muted">Loading…</p> : null}
      {status.recentError ? <p className="text-sm text-danger-ink">Could not load recent tool calls.</p> : null}
      {!status.recentLoading && !status.recentError && status.recent.length === 0 ? (
        <p className="text-sm text-ink-2">No tool calls yet.</p>
      ) : null}
      {status.recent.length > 0 ? (
        <ul className="space-y-1.5">
          {status.recent.map((r) => {
            const b = decisionBadge(r);
            return (
              <li key={r.id} className="flex min-w-0 items-center gap-2 text-xs">
                <Badge tone={b.tone} className="shrink-0">
                  {b.label}
                </Badge>
                <span className="min-w-0 flex-1 truncate font-mono text-ink-2">{r.tool_id ?? `${r.method} ${r.host}`}</span>
                <span className="shrink-0 tabular-nums text-muted">{formatDuration(r.latency_ms)}</span>
              </li>
            );
          })}
        </ul>
      ) : null}
      <Link href={activityHref} className="block text-sm text-link underline">
        All activity
      </Link>
    </div>
  );
}

// ---- Computer ---------------------------------------------------------------------------------------------------

export function ComputerPart({
  sandbox,
  terminalAvailable,
  onNavigate,
}: {
  sandbox: AgentStatus["sandbox"];
  terminalAvailable: boolean;
  onNavigate: (page: AgentPageId) => void;
}) {
  const look = sandbox ? sandboxLook(sandbox) : null;
  return (
    <div className="space-y-2">
      {look ? (
        <StatusDot tone={look.tone} title={look.title}>
          {look.label}
        </StatusDot>
      ) : (
        <p className="text-sm text-muted">Sandbox state unknown</p>
      )}
      <div className="flex flex-wrap gap-2">
        <Button type="button" variant="outline" size="sm" onClick={() => onNavigate("computer")}>
          Open computer
        </Button>
        {terminalAvailable ? (
          <Button type="button" variant="outline" size="sm" onClick={() => onNavigate("terminal")}>
            Terminal
          </Button>
        ) : null}
        {/* (P5-FILES-2, D-316) Files exists exactly where the Terminal does. */}
        {terminalAvailable ? (
          <Button type="button" variant="outline" size="sm" onClick={() => onNavigate("files")}>
            Files
          </Button>
        ) : null}
      </div>
    </div>
  );
}

// ---- About ------------------------------------------------------------------------------------------------------

export function AboutPart({
  kind,
  isDefault,
  brief,
  onNavigate,
}: {
  kind: string;
  isDefault: boolean;
  brief: string | null | undefined;
  onNavigate: (page: AgentPageId) => void;
}) {
  const [open, setOpen] = useState(false);
  return (
    <div className="space-y-2">
      <div className="flex flex-wrap items-center gap-1.5">
        <Badge>{kind}</Badge>
        {isDefault ? <Badge tone="accent">Default</Badge> : null}
      </div>
      {brief ? (
        <p className={open ? "text-sm text-ink-2" : "line-clamp-3 text-sm text-ink-2"}>
          {brief}
          {brief.length > 140 ? (
            <button type="button" className="ml-1 text-link underline" onClick={() => setOpen((o) => !o)} aria-expanded={open}>
              {open ? "Less" : "More"}
            </button>
          ) : null}
        </p>
      ) : (
        <p className="text-sm text-muted">No brief yet.</p>
      )}
      <Button type="button" variant="outline" size="sm" onClick={() => onNavigate("instructions")}>
        Edit instructions
      </Button>
    </div>
  );
}
