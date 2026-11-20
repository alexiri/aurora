"use client";

import { useEffect, useRef, useState, type ReactNode } from "react";

import type { AgentPageId } from "@/components/agents/sections";
import { AttentionChip } from "@/components/ui/attention-chip";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import { ActivityIcon, CheckCircleIcon, ChevronLeftIcon, ChevronRightIcon, WrenchIcon } from "@/components/ui/icons";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import type { Conversation } from "@/lib/api/conversations";
import { cn } from "@/lib/utils";

import { ComputerLine, DoingNow, NeedsYouList, PartHeading, RecentCalls, showApprovalInConversation } from "./parts";
import type { AgentStatus, OpenTurn } from "./status-data";

/** Everything the five parts need; shared by the expanded rail, the collapsed strip's pop-over and the phone sheet. */
export interface RailContext {
  status: AgentStatus;
  open: OpenTurn;
  wsId: string;
  base: string;
  agentId: string;
  agent: { name?: string; kind: string; isDefault: boolean; brief: string | null | undefined };
  terminalAvailable: boolean;
  /** (P4-SSH-2, C-795) The agent's SSH page exists: the Computer part links to it. Optional: absent means off. */
  sshAvailable?: boolean;
  /** The open conversation, for "Show in conversation" and the running tool. */
  openConversation: { id: string; title: string } | null;
  titleOf: (c: Conversation) => string;
  onSelectConversation: (id: string) => void;
  onNavigate: (page: AgentPageId) => void;
}

type PartId = "needs" | "doing" | "did";

const dotTone = { neutral: "bg-muted", accent: "bg-accent", pending: "bg-pending-ink", danger: "bg-danger-ink" } as const;

/** The three parts in order (D-201, P5-REFINE-11): Needs you · Doing now (with the computer's line) · What <Agent> did. `idPrefix` keeps the ids of the rail and the sheet apart. */
function StatusParts({ ctx, idPrefix, onShow }: { ctx: RailContext; idPrefix: string; onShow: (approvalId: string) => void }) {
  const { status } = ctx;
  const activityHref = `${ctx.base}/activity?agent=${encodeURIComponent(ctx.agentId)}`;
  const section = (id: PartId, icon: ReactNode, title: ReactNode, body: ReactNode) => (
    <section aria-labelledby={`${idPrefix}-${id}`} id={`${idPrefix}-part-${id}`} className="space-y-2 scroll-mt-2">
      <PartHeading id={`${idPrefix}-${id}`} icon={icon} pending={id === "needs" && status.needs.length > 0}>
        {title}
      </PartHeading>
      {body}
    </section>
  );
  return (
    <div className="space-y-5">
      {status.canApprove
        ? section(
            "needs",
            <CheckCircleIcon />,
            <>
              Needs you
              {status.needs.length > 0 ? <AttentionChip actionId={`${idPrefix}-needs-list`}>{status.needs.length}</AttentionChip> : null}
            </>,
            <NeedsYouList
              needs={status.needs}
              loading={status.needsLoading}
              wsId={ctx.wsId}
              base={ctx.base}
              openConversationId={ctx.openConversation?.id ?? null}
              onShow={onShow}
            />,
          )
        : null}
      {section(
        "doing",
        <ActivityIcon />,
        "Doing now",
        <div className="space-y-2">
          <DoingNow working={status.working} open={ctx.open} openConversation={ctx.openConversation} titleOf={ctx.titleOf} onSelect={ctx.onSelectConversation} />
          <ComputerLine agentName={ctx.agent.name ?? "the agent"} sandbox={status.sandbox} onNavigate={ctx.onNavigate} />
        </div>,
      )}
      {section("did", <WrenchIcon />, `What ${ctx.agent.name ?? "the agent"} did`, <RecentCalls status={status} activityHref={activityHref} />)}
    </div>
  );
}

/** The collapsed icon's accessible name: the part and its state in words. */
export function stripLabels(ctx: RailContext): Record<PartId, string> {
  const { status } = ctx;
  const n = status.needs.length;
  const working = status.working.length > 0 || ctx.open.active;
  const name = ctx.agent.name ?? "the agent";
  return {
    needs: n === 0 ? "Needs you: nothing waiting" : `Needs you: ${n} ${n === 1 ? "approval" : "approvals"} waiting`,
    doing: working ? "Doing now: working" : "Doing now: idle",
    did: status.problem ? `What ${name} did: some requests were denied or failed` : `What ${name} did: none denied or failed`,
  };
}

const iconButton =
  "group relative inline-flex size-10 shrink-0 items-center justify-center rounded-control text-ink-2 hover:bg-subtle hover:text-ink";

/** (P5-REFINE-11, rec 3) A collapsed part: the icon with a short visible label under it (Need · Now · Did). */
const partButton =
  "group relative inline-flex w-12 shrink-0 flex-col items-center justify-center gap-0.5 rounded-control py-1.5 text-ink-2 hover:bg-subtle hover:text-ink";

function PartLabel({ children }: { children: ReactNode }) {
  return <span aria-hidden="true" className="text-label font-semibold leading-none">{children}</span>;
}

/** The collapsed icon's full name, on hover and keyboard focus (the button keeps its fuller `aria-label`). */
function Tip({ children }: { children: ReactNode }) {
  return (
    <span
      aria-hidden="true"
      className="pointer-events-none absolute right-full z-30 mr-2 hidden whitespace-nowrap rounded-control bg-primary px-2 py-1 text-xs font-medium text-on-primary shadow-pop group-hover:block group-focus-visible:block">
      {children}
    </span>
  );
}

/**
 * The status rail (D-201). Expanded: 288 px with the five parts; collapsed: a 56 px strip of icons with their state.
 * When the chat panel is narrower than 1024 px (`overlay`) the expanded rail covers the thread instead of pushing it;
 * the panel then makes the thread inert (focus stays in the rail) and Esc collapses it.
 */
export function StatusRail({
  ctx,
  expanded,
  onExpandedChange,
  overlay,
}: {
  ctx: RailContext;
  expanded: boolean;
  onExpandedChange: (expanded: boolean) => void;
  overlay: boolean;
}) {
  const { status } = ctx;
  const [needsOpen, setNeedsOpen] = useState(false);
  // Focus moves only after a person's own toggle, never on load.
  const focus = useRef<"collapse" | "expand" | null>(null);
  const part = useRef<PartId | null>(null);
  const collapseRef = useRef<HTMLButtonElement>(null);
  const expandRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    if (focus.current === "collapse") collapseRef.current?.focus();
    if (focus.current === "expand") expandRef.current?.focus();
    focus.current = null;
    if (expanded && part.current) {
      document.getElementById(`rail-part-${part.current}`)?.scrollIntoView?.({ block: "start" });
      part.current = null;
    }
  }, [expanded]);

  useEffect(() => {
    if (!expanded || !overlay) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      focus.current = "expand";
      onExpandedChange(false);
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [expanded, overlay, onExpandedChange]);

  const expandAt = (p: PartId | null) => {
    focus.current = "collapse";
    part.current = p;
    onExpandedChange(true);
  };
  const collapse = () => {
    focus.current = "expand";
    onExpandedChange(false);
  };
  const show = (approvalId: string) => {
    setNeedsOpen(false);
    showApprovalInConversation(approvalId);
    // The overlay covers the thread: get it out of the way.
    if (overlay && expanded) onExpandedChange(false);
  };

  const labels = stripLabels(ctx);
  const working = status.working.length > 0 || ctx.open.active;
  const dot = (tone: keyof typeof dotTone) => (
    <span aria-hidden="true" className={cn("absolute right-1.5 top-1.5 size-2 rounded-full ring-2 ring-surface", dotTone[tone])} />
  );

  const strip = (
    <aside
      aria-label="Status"
      // Behind the overlay the strip is out of reach.
      {...(overlay && expanded ? ({ inert: true } as object) : {})}
      className="flex w-14 shrink-0 flex-col items-center gap-1 border-l border-line pb-2"
    >
      {/* FIX-187: a 64 px top cell like the expanded panel's header; its bottom border is the divider and continues the thread header's. */}
      <div className="mb-1 flex h-16 w-full shrink-0 items-center justify-center border-b border-line">
        <button ref={expandRef} type="button" aria-label="Expand status panel" onClick={() => expandAt(null)} className={iconButton}>
          <ChevronLeftIcon width={20} height={20} />
        </button>
      </div>
      {status.canApprove ? (
        <Popover open={needsOpen} onOpenChange={setNeedsOpen}>
          <PopoverTrigger asChild>
            <button type="button" aria-label={labels.needs} className={partButton}>
              <CheckCircleIcon width={20} height={20} />
              <PartLabel>Need</PartLabel>
              <Tip>Needs you</Tip>
              {status.needs.length > 0 ? (
                <AttentionChip actionId="status-rail-needs" aria-hidden="true" className="absolute -right-1 -top-1 px-1.5 tabular-nums">
                  {status.needs.length}
                </AttentionChip>
              ) : null}
            </button>
          </PopoverTrigger>
          <PopoverContent side="left" align="start" aria-label="Needs you" className="space-y-3">
            <h3 className="flex items-center gap-2 text-sm font-semibold text-ink-2">
              {/* (F1) The amber tile only while something waits. */}
              <span
                aria-hidden="true"
                className={cn("inline-flex", status.needs.length > 0 && "size-6 items-center justify-center rounded-md bg-pending-soft text-pending-ink")}>
                <CheckCircleIcon width={16} height={16} />
              </span>
              Needs you
            </h3>
            <NeedsYouList
              needs={status.needs}
              loading={status.needsLoading}
              wsId={ctx.wsId}
              base={ctx.base}
              openConversationId={ctx.openConversation?.id ?? null}
              onShow={show}
            />
            <button
              type="button"
              className="text-sm text-link underline"
              onClick={() => {
                setNeedsOpen(false);
                expandAt(null);
              }}
            >
              Open the status panel
            </button>
          </PopoverContent>
        </Popover>
      ) : null}
      <button type="button" aria-label={labels.doing} onClick={() => expandAt("doing")} className={partButton}>
        <ActivityIcon width={20} height={20} />
        <PartLabel>Now</PartLabel>
        <Tip>Doing now</Tip>
        {working ? dot("accent") : null}
      </button>
      <button type="button" aria-label={labels.did} onClick={() => expandAt("did")} className={partButton}>
        <WrenchIcon width={20} height={20} />
        <PartLabel>Did</PartLabel>
        <Tip>What {ctx.agent.name ?? "the agent"} did</Tip>
        {status.problem ? dot("danger") : null}
      </button>
    </aside>
  );

  if (!expanded) return strip;

  const panel = (
    <aside
      aria-label="Status"
      className={cn(
        "flex min-h-0 w-72 shrink-0 flex-col border-l border-line bg-surface",
        overlay && "absolute inset-y-0 right-0 z-20 shadow-pop",
      )}
    >
      <header className="flex h-16 shrink-0 items-center justify-between gap-2 border-b border-line px-3">
        <h2 className="font-display text-base font-semibold text-ink">Status</h2>
        <button ref={collapseRef} type="button" aria-label="Collapse status panel" onClick={collapse} className={iconButton}>
          <ChevronRightIcon width={20} height={20} />
        </button>
      </header>
      <div className="min-h-0 flex-1 overflow-y-auto overscroll-contain p-3">
        <StatusParts ctx={ctx} idPrefix="rail" onShow={show} />
      </div>
    </aside>
  );
  // In overlay mode the strip keeps the thread's width stable; the panel lies over it.
  return overlay ? (
    <>
      {strip}
      {panel}
    </>
  ) : (
    panel
  );
}

/** Below 768 px: the same five parts in a sheet (`Dialog`), opened by the header's Status button or the banner. */
export function StatusSheet({ ctx, open, onOpenChange }: { ctx: RailContext; open: boolean; onOpenChange: (open: boolean) => void }) {
  const show = (approvalId: string) => {
    onOpenChange(false);
    // After the dialog has closed and given focus back.
    window.setTimeout(() => showApprovalInConversation(approvalId), 50);
  };
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent title="Status" size="md">
        <StatusParts ctx={ctx} idPrefix="sheet" onShow={show} />
      </DialogContent>
    </Dialog>
  );
}
