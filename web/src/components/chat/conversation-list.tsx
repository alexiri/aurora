"use client";

import { useQueryClient } from "@tanstack/react-query";
import { useState, type ComponentType, type KeyboardEvent, type ReactNode, type SVGProps } from "react";

import { Avatar } from "@/components/agent-avatar";
import { ConversationActions, lifecycleError, refreshConversations } from "@/components/agents/conversation-actions";
import { AttentionChip } from "@/components/ui/attention-chip";
import { Button } from "@/components/ui/button";
import { BotIcon, CalendarIcon, ChevronDownIcon, ChevronRightIcon, EyeIcon, LinkIcon, MessageIcon, PlugIcon, PlusIcon, UsersIcon } from "@/components/ui/icons";
import { InlineSaved, useSaveBar } from "@/components/ui/save-bar";
import { StatusDot } from "@/components/ui/status-dot";
import { Tabs } from "@/components/ui/tabs";
import { chatApi, useConversations, type Conversation, type ConversationFolder } from "@/lib/api/conversations";
import { useStoppedTurns } from "@/lib/chat/stopped";
import { conversationTitle, folderRunsLabel, isOwnDirect, isWorking, kindLabel, listTime, type ListGroups } from "@/lib/chat/layout";
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

/** The round tile of a conversation kind (search results use it at 28 px). */
export function KindTile({ kind, size, className }: { kind: string; size: 28 | 36; className?: string }) {
  const k = KIND[kind] ?? KIND.api;
  const Glyph = k.icon;
  return (
    <span aria-hidden="true" className={cn("inline-flex shrink-0 items-center justify-center rounded-full", size === 28 ? "size-7" : "size-9", k.tint, className)}>
      <Glyph width={size === 28 ? 14 : 18} height={size === 28 ? 14 : 18} />
    </span>
  );
}

const startOfDay = (d: Date) => new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();

/** (chat-3) When it last moved: the clock today, "Yesterday", then the weekday or the date. */
function whenOf(iso: string | null | undefined, now: Date): string {
  if (!iso) return "";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "";
  return Math.round((startOfDay(now) - startOfDay(d)) / 86_400_000) === 1 ? "Yesterday" : listTime(iso, now);
}

/** What the row's second line says besides a status: "You and Daneel · 23:50", "Group · Yesterday". */
function lineOf(c: Conversation, agentName: string | undefined, now: Date): string {
  const at = c.last_activity_at ?? c.created_at;
  const when = whenOf(at, now);
  if (isOwnDirect(c)) {
    // The mockup names the pair on today's rows and gives older ones just the day.
    const d = new Date(at);
    const today = !Number.isNaN(d.getTime()) && startOfDay(now) === startOfDay(d);
    return today || !when ? `You and ${agentName ?? "the agent"}${when ? ` · ${when}` : ""}` : when;
  }
  return when ? `${kindLabel(c.kind)} · ${when}` : kindLabel(c.kind);
}

/** (schedules-7) The amber chip beside the title of a conversation whose schedule or webhook is gone. */
function GoneChip({ kind, actionId }: { kind: string; actionId: string }) {
  // Amber is AttentionChip (D-384): the row itself is the action beside it (open it, or delete it from its menu).
  return (
    <AttentionChip actionId={actionId} className="shrink-0">
      {kind === "webhook" ? "Webhook deleted" : "Schedule deleted"}
    </AttentionChip>
  );
}

/** The row's second line when its automation is gone. */
const goneLine = (kind: string): string => `It was started by a ${kind === "webhook" ? "webhook" : "schedule"} that no longer exists. No new runs.`;

/**
 * A row (chat-3): the kind tile (36 px), the title (a **Rename** turns it into a field: Enter saves, Esc cancels), the
 * second line (status, or who and when) and the row's "…" menu (Rename, Archive, Delete this conversation…).
 */
export function ConversationRow({
  conversation: c,
  selected,
  now,
  onSelect,
  waiting = false,
  agentId,
  agentName,
  onGone,
  nested = false,
}: {
  conversation: Conversation;
  selected: boolean;
  now: Date;
  onSelect: () => void;
  /** (P5-AGENT-2) For approvers: the conversation has a pending approval ("Waiting on you" replaces the kind line). */
  waiting?: boolean;
  /** Offers the row's menu (omit in places that cannot change the conversation). */
  agentId?: string;
  agentName?: string;
  /** The conversation was deleted from the row's menu. */
  onGone?: (id: string) => void;
  /** A run thread inside its automation's folder. */
  nested?: boolean;
}) {
  const qc = useQueryClient();
  const memberDirect = c.kind === "direct" && !!c.direct_user;
  const kind = KIND[c.kind] ?? KIND.api;
  const Glyph = kind.icon;
  const title = conversationTitle(c, now);
  const [editing, setEditing] = useState(false);
  const [value, setValue] = useState("");
  const [err, setErr] = useState<string | null>(null);
  const stoppedTurns = useStoppedTurns();
  const saved = useSaveBar();
  // Only the person's own chats can be renamed (a group's rename is for workspace owners, a run thread follows its automation).
  const canRename = isOwnDirect(c);
  const menu = !!agentId && !memberDirect;

  const save = () => {
    const t = value.trim();
    if (t === title || (t === "" && !c.title)) {
      setEditing(false);
      return;
    }
    chatApi
      .rename(c.id, t === "" ? null : t)
      .then(() => {
        setEditing(false);
        setErr(null);
        if (agentId) refreshConversations(qc, agentId, c.id);
        saved.flash();
      })
      .catch((e: unknown) => setErr(lifecycleError(e)));
  };
  const keys = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "Enter") {
      e.preventDefault();
      save();
    } else if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      setEditing(false);
      setErr(null);
    }
  };

  const second = waiting ? (
    <StatusDot tone="pending" className="whitespace-nowrap text-pending-ink">
      Waiting on you
    </StatusDot>
  ) : isWorking(c, stoppedTurns) ? (
    <StatusDot tone="accent" className="text-accent-ink">
      Working
    </StatusDot>
  ) : memberDirect ? (
    <span className="flex items-center gap-1 text-xs text-muted">
      <EyeIcon aria-hidden="true" width={12} height={12} />
      Read-only
    </span>
  ) : c.automation?.deleted ? (
    <span className="line-clamp-2 break-words text-xs text-muted">{goneLine(c.automation.kind)}</span>
  ) : (
    <span className={cn("text-xs text-muted", nested ? "line-clamp-2 break-words" : "block truncate")}>{lineOf(c, agentName, now)}</span>
  );

  return (
    <div className={cn("flex items-center gap-1 rounded-control hover:bg-subtle", selected && "bg-subtle", nested && "ml-6")}>
      {editing ? (
        <div className="flex min-h-14 min-w-0 flex-1 items-center gap-3 px-2.5 py-2">
          <span aria-hidden="true" className={cn("inline-flex size-9 shrink-0 items-center justify-center rounded-full", kind.tint)}>
            <Glyph width={18} height={18} />
          </span>
          <span className="min-w-0 flex-1">
            <input
              autoFocus
              aria-label="Conversation title"
              aria-invalid={err ? true : undefined}
              className="block w-full min-h-9 rounded-control border border-line-strong bg-surface px-2 text-sm text-ink aria-[invalid=true]:border-danger-ink dark:bg-canvas"
              maxLength={200}
              value={value}
              onChange={(e) => setValue(e.target.value)}
              onKeyDown={keys}
              onBlur={() => {
                setEditing(false);
                setErr(null);
              }}
            />
            {err ? (
              <span role="alert" className="block text-xs text-danger-ink">
                {err}
              </span>
            ) : (
              <span className="block text-xs text-muted">Enter saves · Esc cancels</span>
            )}
          </span>
        </div>
      ) : (
        <button
          type="button"
          id={`conversation-${c.id}`}
          onClick={onSelect}
          aria-current={selected ? "true" : undefined}
          className="flex min-h-14 min-w-0 flex-1 items-center gap-3 rounded-control px-2.5 py-2 text-left">
          {memberDirect ? (
            <Avatar name={c.direct_user?.display_name || "?"} tintKey={c.direct_user?.id} size={36} />
          ) : (
            <span aria-hidden="true" className={cn("inline-flex size-9 shrink-0 items-center justify-center rounded-full", kind.tint)}>
              <Glyph width={18} height={18} />
            </span>
          )}
          <span className="min-w-0 flex-1">
            {/* (schedules-7) The chip is beside the title when there is room and under it when there is not; it is never inside
                the title's clamp, which cut it off in a fixed-width list. A run row keeps its whole title: its time is the point. */}
            <span className="flex flex-wrap items-center gap-x-2 gap-y-1">
              <span className={cn("min-w-0 break-words text-sm text-ink", nested ? "" : "line-clamp-2", selected ? "font-semibold" : "font-medium")}>{title}</span>
              {c.automation?.deleted ? <GoneChip kind={c.automation.kind} actionId={`conversation-${c.id}`} /> : null}
            </span>
            {second}
            {saved.saved ? <InlineSaved show /> : null}
          </span>
        </button>
      )}
      {menu && !editing ? (
        <ConversationActions
          agentId={agentId}
          convId={c.id}
          archived={!!c.archived_at}
          variant="row"
          label={`Actions for ${title}`}
          rename={canRename ? { title, onStart: () => { setValue(c.title ?? title); setErr(null); setEditing(true); } } : undefined}
          onGone={() => onGone?.(c.id)}
        />
      ) : null}
    </div>
  );
}

/**
 * (D-409, C-1074) An automation's folder: "Morning summary · 3 runs", opening to its run threads (each titled with its
 * date). The threads load when the folder opens.
 */
function FolderRow({
  folder,
  agentId,
  selectedId,
  defaultOpen,
  now,
  onSelect,
  onGone,
}: {
  folder: ConversationFolder;
  agentId: string;
  selectedId: string | null;
  defaultOpen: boolean;
  now: Date;
  onSelect: (id: string) => void;
  onGone?: (id: string) => void;
}) {
  const [open, setOpen] = useState(defaultOpen);
  const threads = useConversations(agentId, false, { automation: "only", automationId: folder.id, enabled: open });
  const Glyph = (KIND[folder.kind] ?? KIND.api).icon;
  const tint = (KIND[folder.kind] ?? KIND.api).tint;
  const Chevron = open ? ChevronDownIcon : ChevronRightIcon;
  return (
    <div role="group" aria-label={folderRunsLabel(folder.name, folder.thread_count)}>
      <button
        type="button"
        id={`folder-${folder.id}`}
        aria-expanded={open}
        onClick={() => setOpen((o) => !o)}
        className="flex min-h-14 w-full items-center gap-3 rounded-control px-2.5 py-2 text-left hover:bg-subtle">
        <span aria-hidden="true" className={cn("inline-flex size-9 shrink-0 items-center justify-center rounded-full", tint)}>
          <Glyph width={18} height={18} />
        </span>
        <span className="min-w-0 flex-1">
          {/* A long automation name wraps to a second line; it never cuts the run count off. */}
          <span className="flex flex-wrap items-center gap-x-2 gap-y-1">
            <span className="line-clamp-2 min-w-0 break-words text-sm font-medium text-ink">{folderRunsLabel(folder.name, folder.thread_count)}</span>
            {folder.deleted ? <GoneChip kind={folder.kind} actionId={`folder-${folder.id}`} /> : null}
          </span>
          <span className="line-clamp-2 break-words text-xs text-muted">
            {folder.deleted ? goneLine(folder.kind) : `${folder.kind === "webhook" ? "Webhook" : "Schedule"} · ${whenOf(folder.last_activity_at, now)}`}
          </span>
        </span>
        <Chevron aria-hidden="true" width={16} height={16} className="shrink-0 text-muted" />
      </button>
      {open ? (
        <div className="space-y-0.5">
          {threads.isPending ? <p className="py-2 pl-8 text-xs text-muted">Loading runs…</p> : null}
          {(threads.data?.items ?? []).map((c) => (
            <ConversationRow key={c.id} conversation={c} selected={selectedId === c.id} now={now} onSelect={() => onSelect(c.id)} agentId={agentId} onGone={onGone} nested />
          ))}
        </div>
      ) : null}
    </div>
  );
}

/** The list pane: heading with New chat, title filter, Active/Archived switch and the rows. The owners' group is caps-labelled. */
export function ConversationList({
  groups,
  folders = [],
  selectedId,
  selectedAutomationId = null,
  archived,
  onArchivedChange,
  searchBox,
  results,
  query,
  onSelect,
  onGone,
  now,
  status,
  className,
  waitingIds,
  agentId,
  agentName,
  onNewChat,
  newChatBusy = false,
  newChatError,
}: {
  /** The agent whose conversations these are: the rows' menus and the folders load by it. */
  agentId?: string;
  /** (chat-1) For the empty Active list: "Start a new chat to talk with <agent>." and the New chat button. */
  agentName?: string;
  /** (D-409) The **New chat** button: a fresh conversation with empty history. */
  onNewChat?: () => void;
  newChatBusy?: boolean;
  /** Why New chat failed (the limit of chats), in words. */
  newChatError?: string | null;
  /** (C-1074) The agent's schedules and webhooks that have run threads (Active list only). */
  folders?: ConversationFolder[];
  /** The selected conversation's automation: its folder opens. */
  selectedAutomationId?: string | null;
  /** A conversation was deleted from a row's menu. */
  onGone?: (id: string) => void;
  /** (P5-CHAT-4) The search box with its chips; it replaces the title filter. */
  searchBox: ReactNode;
  /** (P5-CHAT-4) While a message search shows, its results replace the Active/Archived switch and the rows. */
  results?: ReactNode;
  /** The words in the box, only for the "No conversations match" line. */
  /** (P5-AGENT-2) Ids of conversations with a pending approval, for approvers only (omit for everyone else). */
  waitingIds?: ReadonlySet<string>;
  groups: ListGroups;
  /** The selected conversation id, or `null` when none is picked. */
  selectedId: string | null;
  archived: boolean;
  onArchivedChange: (archived: boolean) => void;
  query: string;
  onSelect: (id: string) => void;
  now: Date;
  /** A loading or error state of the list query, shown above the rows. */
  status?: ReactNode;
  className?: string;
}) {
  const { mine, others, memberDirects } = groups;
  const shownFolders = archived ? [] : folders;
  const rows = (items: Conversation[]) =>
    items.map((c) => (
      <ConversationRow
        key={c.id}
        conversation={c}
        selected={selectedId === c.id}
        now={now}
        onSelect={() => onSelect(c.id)}
        waiting={waitingIds?.has(c.id) ?? false}
        agentId={agentId}
        agentName={agentName}
        onGone={onGone}
      />
    ));
  const empty = mine.length === 0 && others.length === 0 && memberDirects.length === 0 && shownFolders.length === 0;
  return (
    <nav aria-label="Conversations" className={cn("flex min-h-0 flex-col gap-3 p-3", className)}>
      <div className="flex items-center justify-between gap-2">
        <h2 className="font-display text-base font-semibold text-ink">Conversations</h2>
        {onNewChat ? (
          <Button type="button" variant="primary" size="sm" className="min-h-11 shrink-0 md:min-h-9" disabled={newChatBusy} onClick={onNewChat}>
            <PlusIcon aria-hidden="true" width={16} height={16} />
            New chat
          </Button>
        ) : null}
      </div>
      {newChatError ? (
        <p role="alert" className="text-sm text-danger-ink">
          {newChatError}
        </p>
      ) : null}
      {searchBox}
      {results ? (
        results
      ) : (
        <>
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
      <div
        role="tabpanel"
        id={`chat-list-panel-${archived ? "archived" : "active"}`}
        aria-labelledby={`chat-list-tab-${archived ? "archived" : "active"}`}
        className="-mx-1 min-h-0 flex-1 space-y-0.5 overflow-y-auto px-1">
        {status}
        {empty && !status && !query.trim() && !archived && onNewChat ? (
          <div className="space-y-2 rounded-card border border-dashed border-line-strong p-3">
            <p className="text-sm font-semibold text-ink">No active conversations</p>
            <p className="text-sm text-ink-2">Start a new chat to talk with {agentName ?? "the agent"}.</p>
            <Button type="button" variant="primary" size="sm" className="min-h-11 md:min-h-9" disabled={newChatBusy} onClick={onNewChat}>
              New chat
            </Button>
          </div>
        ) : empty && !status ? (
          <p className="px-2 py-3 text-sm text-muted">
            {query.trim() ? "No conversations match." : archived ? "No archived conversations." : "No conversations yet."}
          </p>
        ) : null}
        {rows(mine)}
        {rows(others)}
        {shownFolders.length > 0 && agentId ? (
          <div role="group" aria-label="Automations" className="space-y-0.5 pt-3">
            <p className="px-2.5 pb-1 text-label font-semibold uppercase tracking-label text-muted">Automations</p>
            {shownFolders.map((f) => (
              <FolderRow
                key={`${f.kind}:${f.id}`}
                folder={f}
                agentId={agentId}
                selectedId={selectedId}
                defaultOpen={selectedAutomationId === f.id}
                now={now}
                onSelect={onSelect}
                onGone={onGone}
              />
            ))}
          </div>
        ) : null}
        {memberDirects.length > 0 ? (
          <div role="group" aria-label="Other members' direct chats" className="space-y-0.5 pt-3">
            <p className="px-2.5 pb-1 text-label font-semibold uppercase tracking-label text-muted">Other members&apos; direct chats</p>
            {rows(memberDirects)}
          </div>
        ) : null}
      </div>
        </>
      )}
    </nav>
  );
}
