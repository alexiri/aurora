"use client";

import Link from "next/link";
import { useState, type ComponentType, type ReactNode, type SVGProps } from "react";

import { DeclineRequestDialog } from "@/components/attention/decline-dialog";
import { AttentionHistory, askedNoun } from "@/components/attention/needs-you-feed";
import { AttentionChip } from "@/components/ui/attention-chip";
import { Badge } from "@/components/ui/badge";
import { Button, buttonVariants } from "@/components/ui/button";
import { ArrowRightIcon, MailIcon, MessageIcon, PlugIcon, SparkIcon, UsersIcon } from "@/components/ui/icons";
import { ItemList } from "@/components/ui/section";
import { useDeclineSetup } from "@/lib/api/app-setup-requests";
import { useAttentionFeed, type AttentionItem } from "@/lib/api/attention";
import { useInvitations } from "@/lib/api/org";
import { orgRoutes, TOPIC_LABEL } from "@/lib/routes";
import { cn } from "@/lib/utils";
import { personName } from "@/lib/workspace-name";

type Icon = ComponentType<SVGProps<SVGSVGElement>>;

/** The page a feed item's `topic` belongs to, in the menu's words and with its mark. */
const TOPICS: Record<string, { label: string; icon: Icon }> = {
  apps: { label: TOPIC_LABEL.apps, icon: PlugIcon },
  accounts: { label: TOPIC_LABEL.apps, icon: PlugIcon },
  "chat-platforms": { label: TOPIC_LABEL["chat-platforms"], icon: MessageIcon },
  "ai-models": { label: TOPIC_LABEL["ai-models"], icon: SparkIcon },
  skills: { label: TOPIC_LABEL.skills, icon: SparkIcon },
  people: { label: TOPIC_LABEL.people, icon: UsersIcon },
};

/** Something that stopped working reads as danger; everything else waits for an answer. */
const BROKEN: ReadonlySet<AttentionItem["kind"]> = new Set(["broken_account", "org_account_broken", "connector_down", "agent_cannot_answer"]);

/** (nav-3b) Kinds whose row ends in a word for what is the matter, not in a button: the whole row opens the page. */
const STATUS: Partial<Record<AttentionItem["kind"], string>> = { library_update: "Review", org_account_broken: "Failed" };

function Status({ word, broken, itemId }: { word: string; broken: boolean; itemId: string }) {
  // Amber names the thing it is about: the row's own link.
  return broken ? <Badge tone="danger">{word}</Badge> : <AttentionChip actionId={itemId}>{word}</AttentionChip>;
}

function Row({
  icon: Mark,
  broken,
  title,
  detail,
  href,
  linkId,
  actions,
  ...rest
}: {
  icon: Icon;
  broken?: boolean;
  title: ReactNode;
  detail: ReactNode;
  href: string;
  linkId?: string;
  actions?: ReactNode;
} & Record<`data-${string}`, string | undefined>) {
  return (
    <li {...rest} className="relative flex items-center gap-3 px-4 py-3 text-sm hover:bg-subtle">
      <span className={cn("inline-flex size-8 shrink-0 items-center justify-center rounded-control", broken ? "bg-danger-soft text-danger-ink" : "bg-pending-soft text-pending-ink")}>
        <Mark className="size-4" />
      </span>
      {/* On a phone the controls wrap under the words; the mark and the arrow stay at the row's two ends. */}
      <div className="flex min-w-0 flex-1 flex-wrap items-center gap-x-3 gap-y-2">
        <span className="min-w-0 flex-1 basis-48">
          {/* The whole row opens the item's page; the row's own controls sit above this link's hit area. */}
          <Link id={linkId} href={href} className="block font-semibold text-ink after:absolute after:inset-0">
            {title}
          </Link>
          <span className="block text-xs text-muted">{detail}</span>
        </span>
        {actions ? <span className="relative flex shrink-0 flex-wrap items-center gap-2">{actions}</span> : null}
      </div>
      <ArrowRightIcon aria-hidden="true" className="size-4 shrink-0 text-ink-2" />
    </li>
  );
}

function ItemRow({
  item,
  onDecline,
  declining,
  declineError,
}: {
  item: AttentionItem;
  onDecline: (message: string | undefined, done: () => void) => void;
  declining: boolean;
  declineError: unknown;
}) {
  const [open, setOpen] = useState(false);
  const topic = TOPICS[item.topic] ?? { label: undefined, icon: PlugIcon };
  const status = STATUS[item.kind];
  const linkId = status ? `attention-link-${item.id.replace(/[^a-zA-Z0-9-]/g, "-")}` : undefined;
  return (
    <Row
      data-kind={item.kind}
      data-testid={`attention-item-${item.id}`}
      icon={topic.icon}
      broken={BROKEN.has(item.kind)}
      title={item.title}
      detail={
        <>
          {topic.label ? `${topic.label} · ` : ""}
          {item.detail}
          {item.reason ? <span className="mt-0.5 block">“{item.reason}”</span> : null}
        </>
      }
      href={item.href}
      linkId={linkId}
      actions={
        status ? (
          <Status word={status} broken={BROKEN.has(item.kind)} itemId={linkId!} />
        ) : (
          <>
            {item.action ? (
              <Link href={item.action.href} aria-label={`${item.action.label}: ${item.title}`} className={buttonVariants({ variant: "outline", size: "sm" })}>
                {item.action.label}
              </Link>
            ) : null}
            {item.can_decline ? (
              <>
                <Button variant="ghost" size="sm" aria-label={`Decline: ${item.title}`} onClick={() => setOpen(true)}>
                  Decline
                </Button>
                <DeclineRequestDialog
                  open={open}
                  onOpenChange={setOpen}
                  names={item.actors.map((a) => personName(a.display_name))}
                  noun={askedNoun(item)}
                  busy={declining}
                  error={declineError}
                  onDecline={(message) => onDecline(message, () => setOpen(false))}
                />
              </>
            ) : null}
          </>
        )
      }
    />
  );
}

/**
 * (P5-REFINE-17 #16, C-1080; D-508(j), nav-3b) **Needs you** on Manage <org> › Overview: the items behind Manage's count
 * (only what an owner or admin can act on), each opening its page; then, not counted, the invitations still waiting for an
 * answer; **History** keeps what closed in the last 30 days.
 */
export function NeedsYouOrg({ orgId, orgSlug }: { orgId: string; orgSlug: string }) {
  const feed = useAttentionFeed("organization", orgId);
  const decline = useDeclineSetup(orgId);
  const invites = useInvitations(orgId);
  const waiting = invites.data?.items.length ?? 0;
  const items = (feed.data?.items ?? []).filter((i) => !i.own);
  const count = feed.data?.count ?? 0;
  return (
    <>
      {items.length > 0 || waiting > 0 ? (
        <section aria-labelledby="org-needs-you-h" data-testid="org-needs-you" className="space-y-3">
          <h2 id="org-needs-you-h" className="font-display text-base font-semibold text-ink">
            Needs you{" "}
            {count > 0 ? (
              <AttentionChip actionId="org-needs-you-list" aria-label={`${count} waiting`}>
                {count}
              </AttentionChip>
            ) : null}
          </h2>
          <ItemList id="org-needs-you-list" aria-label="Waiting for you">
            {items.map((i) => (
              <ItemRow
                key={i.id}
                item={i}
                declining={decline.isPending}
                declineError={decline.error}
                onDecline={(message, done) => decline.mutate({ id: i.id, message }, { onSuccess: done })}
              />
            ))}
            {waiting > 0 ? (
              <Row
                icon={MailIcon}
                title={`${waiting} ${waiting === 1 ? "invitation is" : "invitations are"} waiting for an answer.`}
                detail={TOPIC_LABEL.people}
                href={orgRoutes(orgSlug).people()}
              />
            ) : null}
          </ItemList>
        </section>
      ) : null}
      <AttentionHistory level="organization" id={orgId} />
    </>
  );
}
