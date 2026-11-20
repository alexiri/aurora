"use client";

import type { ReactNode } from "react";

import { ReassignPanel } from "@/components/automations/owner-controls";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import { ClockIcon, LinkIcon, MessageIcon } from "@/components/ui/icons";
import { Menu, MenuContent, MenuTrigger } from "@/components/ui/menu";

export type AutomationKind = "schedule" | "webhook" | "chat";

const ICON = { schedule: ClockIcon, webhook: LinkIcon, chat: MessageIcon } as const;
const KIND_WORD = { schedule: "Schedule", webhook: "Webhook", chat: "Chat" } as const;

/** (P5-REFINE-14, D-407) The state in words: **On** or **Paused**, nothing else. */
export function StateBadge({ on }: { on: boolean }) {
  return <Badge tone={on ? "accent" : "neutral"}>{on ? "On" : "Paused"}</Badge>;
}

/**
 * One row of the Automations list: kind icon, name, state, then what the row says in words (`when`, `body`, `meta`), the
 * notices, and the actions (primary action, Edit, and the "…" menu). The kind is named for assistive technology only; the icon
 * is decoration.
 */
export function RowShell({
  kind,
  name,
  on,
  when,
  body,
  meta,
  notices,
  actions,
  children,
}: {
  kind: AutomationKind;
  name: string;
  on: boolean;
  when: ReactNode;
  body?: ReactNode;
  meta?: ReactNode;
  notices?: ReactNode;
  actions: ReactNode;
  children?: ReactNode;
}) {
  const Icon = ICON[kind];
  return (
    <li className="space-y-3 p-4" data-automation-kind={kind}>
      <div className="flex items-start gap-3">
        <span aria-hidden="true" className="mt-0.5 flex size-8 shrink-0 items-center justify-center rounded-control bg-subtle text-ink-2 [&_svg]:size-4">
          <Icon />
        </span>
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
            <h3 className="min-w-0 break-words font-display text-base font-semibold text-ink">
              <span className="sr-only">{KIND_WORD[kind]}: </span>
              {name}
            </h3>
            <StateBadge on={on} />
          </div>
          <p className="mt-0.5 text-sm text-ink-2">{when}</p>
        </div>
      </div>
      {body ? <p className="whitespace-pre-wrap break-words text-sm text-ink-2">{body}</p> : null}
      {meta ? <p className="text-xs text-muted">{meta}</p> : null}
      {notices}
      <div className="flex flex-wrap items-center gap-2">{actions}</div>
      {children}
    </li>
  );
}

/** The "…" button and its menu; `children` are `MenuItem`s. */
export function RowMenu({ name, children }: { name: string; children: ReactNode }) {
  return (
    <Menu>
      <MenuTrigger asChild>
        <Button size="sm" variant="outline" className="pointer-coarse:min-h-11" aria-label={`More for ${name}`}>
          <span aria-hidden="true">…</span>
        </Button>
      </MenuTrigger>
      <MenuContent align="end">{children}</MenuContent>
    </Menu>
  );
}

/** Reassign opens as a dialog (it no longer opens above the row and pushes its button away). */
export function ReassignDialog({
  open,
  onOpenChange,
  agentId,
  wsId,
  kind,
  id,
  name,
  creatorId,
  disabledReason,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  agentId: string;
  wsId: string;
  kind: "schedule" | "webhook";
  id: string;
  name: string;
  creatorId?: string;
  disabledReason?: string | null;
}) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent title={`Reassign ${name}`} description="Pick a member of the workspace who takes it over.">
        <ReassignPanel
          agentId={agentId}
          wsId={wsId}
          kind={kind}
          id={id}
          name={name}
          creatorId={creatorId}
          disabledReason={disabledReason}
          onClose={() => onOpenChange(false)}
        />
      </DialogContent>
    </Dialog>
  );
}
