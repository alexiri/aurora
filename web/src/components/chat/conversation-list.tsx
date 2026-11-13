"use client";

import type { ComponentType, ReactNode, SVGProps } from "react";

import { Avatar } from "@/components/agent-avatar";
import { BotIcon, CalendarIcon, EyeIcon, LinkIcon, MessageIcon, PlugIcon, SearchIcon, UsersIcon } from "@/components/ui/icons";
import { StatusDot } from "@/components/ui/status-dot";
import { Tabs } from "@/components/ui/tabs";
import type { Conversation } from "@/lib/api/conversations";
import { conversationTitle, isWorking, kindLabel, listTime, type ListGroups } from "@/lib/chat/layout";
import { cn } from "@/lib/utils";

type Icon = ComponentType<SVGProps<SVGSVGElement>>;

/** One icon and tint per conversation kind. */
const KIND: Record<string, { icon: Icon; tint: string }> = {
  direct: { icon: MessageIcon, tint: "bg-accent-soft text-accent-ink" },
  group: { icon: UsersIcon, tint: "bg-avatar-blue text-avatar-blue-ink" },
  schedule: { icon: CalendarIcon, tint: "bg-pending-soft text-pending-ink" },
  webhook: { icon: LinkIcon, tint: "bg-avatar-lavender text-avatar-lavender-ink" },
  watch: { icon: EyeIcon, tint: "bg-avatar-blue text-avatar-blue-ink" },
  peer: { icon: BotIcon, tint: "bg-accent-soft text-accent-ink" },
  api: { icon: PlugIcon, tint: "bg-subtle text-ink-2" },
};

/** A row: the kind tile (36 px), the title, the kind line or the Working dot, and the last activity on the right. */
export function ConversationRow({
  conversation: c,
  selected,
  now,
  onSelect,
}: {
  conversation: Conversation;
  selected: boolean;
  now: Date;
  onSelect: () => void;
}) {
  const memberDirect = c.kind === "direct" && !!c.direct_user;
  const kind = KIND[c.kind] ?? KIND.api;
  const Glyph = kind.icon;
  return (
    <button
      type="button"
      onClick={onSelect}
      aria-current={selected ? "true" : undefined}
      className={cn(
        "flex min-h-14 w-full items-center gap-3 rounded-control px-2.5 py-2 text-left hover:bg-subtle",
        selected && "bg-subtle",
      )}
    >
      {memberDirect ? (
        <Avatar name={c.direct_user?.display_name || "?"} tintKey={c.direct_user?.id} size={36} />
      ) : (
        <span aria-hidden="true" className={cn("inline-flex size-9 shrink-0 items-center justify-center rounded-full", kind.tint)}>
          <Glyph width={18} height={18} />
        </span>
      )}
      <span className="min-w-0 flex-1">
        <span className={cn("block truncate text-sm text-ink", selected ? "font-semibold" : "font-medium")}>{conversationTitle(c)}</span>
        {isWorking(c) ? (
          <StatusDot tone="accent" className="text-accent-ink">
            Working
          </StatusDot>
        ) : memberDirect ? (
          <span className="flex items-center gap-1 text-xs text-muted">
            <EyeIcon aria-hidden="true" width={12} height={12} />
            Read-only
          </span>
        ) : (
          <span className="block truncate text-xs text-muted">{c.kind === "direct" && !c.title ? "You and the agent" : kindLabel(c.kind)}</span>
        )}
      </span>
      <span className="shrink-0 self-start pt-0.5 text-xs tabular-nums text-muted">{listTime(c.last_activity_at, now)}</span>
    </button>
  );
}

/** The list pane: heading, title filter, Active/Archived switch and the rows. The owners' group is caps-labelled. */
export function ConversationList({
  groups,
  selectedId,
  archived,
  onArchivedChange,
  query,
  onQueryChange,
  onSelect,
  now,
  status,
  className,
}: {
  groups: ListGroups;
  /** The selected conversation id, or `null` when none is picked. */
  selectedId: string | null;
  archived: boolean;
  onArchivedChange: (archived: boolean) => void;
  query: string;
  onQueryChange: (q: string) => void;
  onSelect: (id: string) => void;
  now: Date;
  /** A loading or error state of the list query, shown above the rows. */
  status?: ReactNode;
  className?: string;
}) {
  const { mine, others, memberDirects } = groups;
  const rows = (items: Conversation[]) =>
    items.map((c) => <ConversationRow key={c.id} conversation={c} selected={selectedId === c.id} now={now} onSelect={() => onSelect(c.id)} />);
  const empty = !mine && others.length === 0 && memberDirects.length === 0;
  return (
    <nav aria-label="Conversations" className={cn("flex min-h-0 flex-col gap-3 p-3", className)}>
      <h2 className="font-display text-base font-semibold text-ink">Conversations</h2>
      <div className="relative">
        <SearchIcon aria-hidden="true" className="pointer-events-none absolute left-3 top-1/2 -translate-y-1/2 text-muted" />
        <input
          type="search"
          aria-label="Filter conversations"
          placeholder="Filter by title"
          value={query}
          onChange={(e) => onQueryChange(e.target.value)}
          className="min-h-11 w-full rounded-control border border-line-strong bg-surface pl-9 pr-3 text-sm text-ink placeholder:text-muted md:min-h-9 dark:bg-canvas"
        />
      </div>
      <Tabs
        label="Conversation state"
        idBase="chat-list"
        items={[
          { value: "active", label: "Active" },
          { value: "archived", label: "Archived" },
        ]}
        value={archived ? "archived" : "active"}
        onValueChange={(v) => onArchivedChange(v === "archived")}
        className="w-full [&>button]:flex-1 [&>button]:justify-center [&>button]:min-h-11 md:[&>button]:min-h-9"
      />
      <div role="tabpanel" id={`chat-list-panel-${archived ? "archived" : "active"}`} aria-labelledby={`chat-list-tab-${archived ? "archived" : "active"}`} className="-mx-1 min-h-0 flex-1 space-y-0.5 overflow-y-auto px-1">
        {status}
        {empty && !status ? (
          <p className="px-2 py-3 text-sm text-muted">
            {query.trim() ? "No conversations match." : archived ? "No archived conversations." : "No conversations yet."}
          </p>
        ) : null}
        {mine ? rows([mine]) : null}
        {rows(others)}
        {memberDirects.length > 0 ? (
          <div role="group" aria-label="Other members' direct chats" className="space-y-0.5 pt-3">
            <p className="px-2.5 pb-1 text-label font-semibold uppercase tracking-label text-muted">Other members&apos; direct chats</p>
            {rows(memberDirects)}
          </div>
        ) : null}
      </div>
    </nav>
  );
}
