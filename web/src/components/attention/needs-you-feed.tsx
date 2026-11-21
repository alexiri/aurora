"use client";

import Link from "next/link";
import { useState, type ReactNode } from "react";

import { Avatar } from "@/components/ui/avatar";
import { Button, buttonVariants } from "@/components/ui/button";
import { AttentionChip } from "@/components/ui/attention-chip";
import { formatWhen } from "@/lib/format-date";
import { useAttentionFeed, type AttentionFeedState, type AttentionItem, type AttentionLevel } from "@/lib/api/attention";
import { cn } from "@/lib/utils";

import { personName } from "@/lib/workspace-name";
import { DeclineRequestDialog } from "./decline-dialog";

/** What was asked for, in a phrase, from a request item's title ("Bob Builder wants Jira" → "Jira"). */
export function askedNoun(item: Pick<AttentionItem, "title" | "kind">): string {
  const m = /\b(?:wants?|want) (.+)$/.exec(item.title);
  if (m?.[1]) return m[1];
  return item.kind === "chat_platform_request" ? "a chat platform" : item.kind === "ai_key_request" ? "an AI key" : "something";
}

/**
 * (P5-REFINE-17 #16, P5-REFINE-18 #13, C-1080) One row of a level's attention feed, the same shape everywhere: the asker's
 * avatar, the title, the detail (and the asker's reason in quotes), the one primary action and, on a request the viewer may
 * decline, **Decline**. Text pointing to an administrator names the role (D-407); that is the server's sentence.
 */
export function AttentionRow({
  item,
  onDecline,
  declining,
  declineError,
}: {
  item: AttentionItem;
  /** Declines a request item (its id is the request id). Absent: no Decline button. */
  onDecline?: (item: AttentionItem, message: string | undefined, done: () => void) => void;
  declining?: boolean;
  declineError?: unknown;
}) {
  const [open, setOpen] = useState(false);
  const who = item.actors[0];
  const names = item.actors.map((a) => personName(a.display_name));
  return (
    <li data-kind={item.kind} data-testid={`attention-item-${item.id}`} className="flex flex-wrap items-center gap-x-3 gap-y-2 rounded-control bg-surface px-3 py-2.5 text-sm">
      {who ? <Avatar name={personName(who.display_name)}tintKey={who.user_id} size={32} /> : null}
      <span className="min-w-0 flex-1 basis-56">
        <span className="block font-semibold text-ink">{item.title}</span>
        <span className="block text-ink-2">{item.detail}</span>
        {item.reason ? <span className="mt-0.5 block text-xs text-muted">“{item.reason}”</span> : null}
      </span>
      <span className="flex shrink-0 flex-wrap items-center gap-2">
        {item.action ? (
          <Link
            href={item.action.href}
            aria-label={`${item.action.label}: ${item.title}`}
            className={cn(buttonVariants({ variant: item.decision ? "primary" : "outline", size: "sm" }), item.decision && "text-on-primary!")}
          >
            {item.action.label}
          </Link>
        ) : null}
        {item.can_decline && onDecline ? (
          <Button variant="ghost" size="sm" aria-label={`Decline: ${item.title}`} onClick={() => setOpen(true)}>
            Decline
          </Button>
        ) : null}
      </span>
      {item.can_decline && onDecline ? (
        <DeclineRequestDialog
          open={open}
          onOpenChange={setOpen}
          names={names}
          noun={askedNoun(item)}
          busy={declining}
          error={declineError}
          onDecline={(message) => onDecline(item, message, () => setOpen(false))}
        />
      ) : null}
    </li>
  );
}

/**
 * (P5-REFINE-20, setup-5) A row of the same shape as [`AttentionRow`] for something that is not a feed item: a fact read from
 * the setup status (apps people cannot sign in to, e-mail not set up). It sits in the one Needs you list as an `extra`, not counted.
 */
export function NeedsYouRow({ icon, title, detail, action, ...rest }: { icon?: ReactNode; title: ReactNode; detail?: ReactNode; action: ReactNode } & Record<`data-${string}`, string | undefined>) {
  return (
    <li {...rest} className="flex flex-wrap items-center gap-x-3 gap-y-2 rounded-control bg-surface px-3 py-2.5 text-sm">
      {icon}
      <span className="min-w-0 flex-1 basis-56">
        <span className="block font-semibold text-ink">{title}</span>
        {detail ? <span className="block text-ink-2">{detail}</span> : null}
      </span>
      <span className="flex shrink-0 flex-wrap items-center gap-2">{action}</span>
    </li>
  );
}

/**
 * The **Needs you** card of an Overview at the organization or Administration level: the level's feed in one shape, with
 * the count (C-1080 `count`: open items that are not the viewer's own). `extra` rows (informational, not counted) follow
 * the feed. Nothing at all when nothing waits and there is no extra row.
 */
export function NeedsYouCard({
  id,
  items,
  count,
  onDecline,
  declining,
  declineError,
  extra,
}: {
  id: string;
  items: AttentionItem[];
  count: number;
  onDecline?: (item: AttentionItem, message: string | undefined, done: () => void) => void;
  declining?: boolean;
  declineError?: unknown;
  extra?: ReactNode;
}) {
  if (items.length === 0 && !extra) return null;
  return (
    <section aria-labelledby={`${id}-h`} data-testid={id} className="rounded-card border border-pending-line bg-pending-panel p-4 text-ink sm:p-5">
      <h2 id={`${id}-h`} className="font-display text-base font-semibold text-pending-ink">
        Needs you{" "}
        {count > 0 ? (
          <AttentionChip actionId={`${id}-list`} aria-label={`${count} waiting`}>
            {count}
          </AttentionChip>
        ) : null}
      </h2>
      <ul className="mt-3 space-y-2" aria-label="Waiting for you">
        {items.map((i) => (
          <AttentionRow key={i.id} item={i} onDecline={onDecline} declining={declining} declineError={declineError} />
        ))}
        {extra}
      </ul>
    </section>
  );
}

/** "Declined" / "Done" with when, who by role (D-407) and the decline message: one closed item of the 30-day history. */
function HistoryRow({ item }: { item: AttentionItem }) {
  const role = item.closed_by_role === "instance_admin" ? "an instance admin" : item.closed_by_role === "org_admin" ? "an organization admin" : null;
  const by = item.closed_by ? personName(item.closed_by.display_name) : role;
  const verb = item.state === "declined" ? "Declined" : "Done";
  return (
    <li data-testid={`attention-history-${item.id}`} className="rounded-control bg-surface px-3 py-2 text-sm">
      <span className="block font-medium text-ink">{item.title}</span>
      <span className="block text-xs text-muted">
        {verb}
        {by ? ` by ${by}` : ""}
        {item.closed_at ? ` · ${formatWhen(item.closed_at)}` : ""}
      </span>
      {item.message ? <span className="mt-0.5 block text-xs text-ink-2">“{item.message}”</span> : null}
    </li>
  );
}

/**
 * (setup-8, C-1080 `state=done`) What closed in the last 30 days at a level, under a fold that fetches only when opened.
 */
export function AttentionHistory({ level, id, enabled = true }: { level: AttentionLevel; id?: string; enabled?: boolean }) {
  const [opened, setOpened] = useState(false);
  const state: AttentionFeedState = "done";
  const q = useAttentionFeed(level, id, state, enabled && opened);
  const items = q.data?.items ?? [];
  return (
    <details className="rounded-card border border-line bg-surface p-4" onToggle={(e) => setOpened((e.currentTarget as HTMLDetailsElement).open)}>
      <summary className="cursor-pointer font-display text-base font-semibold text-ink">History</summary>
      <div className="mt-3 space-y-2">
        <p className="text-sm text-ink-2">Requests that were set up or declined in the last 30 days.</p>
        {opened && q.isPending ? <p className="text-sm text-muted">Loading…</p> : null}
        {opened && q.isError ? <p className="text-sm text-danger-ink">Couldn&apos;t load the history.</p> : null}
        {opened && q.isSuccess && items.length === 0 ? <p className="text-sm text-muted">Nothing closed in the last 30 days.</p> : null}
        {items.length > 0 ? (
          <ul className="space-y-2" aria-label="Closed in the last 30 days">
            {items.map((i) => (
              <HistoryRow key={`${i.id}:${i.closed_at ?? ""}`} item={i} />
            ))}
          </ul>
        ) : null}
      </div>
    </details>
  );
}
