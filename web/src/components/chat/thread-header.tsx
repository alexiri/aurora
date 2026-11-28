"use client";

import { Fragment, type ReactNode } from "react";

import { Badge } from "@/components/ui/badge";
import { ArrowLeftIcon, LockIcon, UsersIcon } from "@/components/ui/icons";
import { InfoTip } from "@/components/ui/popover";
import { useMediaQuery } from "@/lib/use-media-query";

type Segment = string | { text: string; className?: string };
const textOf = (s: Segment) => (typeof s === "string" ? s : s.text);
const classOf = (s: Segment) => (typeof s === "string" ? undefined : s.className);
const NO_BREAK_SPACE = String.fromCharCode(0xa0);

/**
 * The dot after a meta segment, held to it by a no-break space: where the line wraps, no row starts with a dot. It takes
 * the class of the segment it leads to, so it hides with it.
 */
function Separator({ className }: { className?: string }) {
  return (
    <span aria-hidden="true" className={className}>
      {`${NO_BREAK_SPACE}·`}
    </span>
  );
}

/**
 * The thread header: a 44 px back button on phones, the title, one muted meta line (visibility, kind, archived date,
 * retention window), the connection pill and the actions slot.
 *
 * Below 1280 px the meta line has its own full-width row under the title row: the title row is 64 px, so the row's border
 * stays level with the status panel's header (P5-POLISH-6), and the whole line (the privacy note, "Archived … · read only")
 * wraps there in full, so nothing needs a hover. From 1280 px it sits under the title, inside the 64 px header.
 *
 * (retention-tooltip) When the org has a retention window, the note ("Deleted after N days without activity") stays
 * inline below 1280 px; from 1280 px it is the visibility line's tooltip (hover, focus or tap opens it), so the line
 * still fits the pinned 64 px header (agent-page-p5) and desktop never loses the note.
 */
export function ThreadHeader({
  title,
  visibility,
  visibilityIcon = "lock",
  retention,
  segments,
  live,
  onBack,
  actions,
}: {
  title: string;
  /** The visibility note (D-479's line, every conversation). */
  visibility?: string;
  /** (FIX-235, D-479) The note's icon: the lock for direct chats, the people icon for the others. */
  visibilityIcon?: "lock" | "people";
  /** (retention-tooltip) The retention note; from 1280 px the visibility line's tooltip, below it the inline segment. */
  retention?: string;
  /** The other meta segments (kind, archived date, retention). A segment may carry a class —
   * the retention one hides at xl to keep the 64 px header (agent-page-p5 pins that height). */
  segments: Segment[];
  /** `true` Live, `false` Reconnecting…, `null` no pill (loading or failed). */
  live: boolean | null;
  onBack?: () => void;
  actions?: ReactNode;
}) {
  const hasMeta = !!visibility || segments.length > 0;
  // (retention-tooltip) The tooltip exists only from 1280 px: below it the note stays an inline segment, and the
  // visibility line stays the plain text it is today (one `matchMedia`-only decision, never duplicated markup).
  const xl = useMediaQuery("(min-width: 1280px)");
  return (
    <header
      className={
        "flex shrink-0 flex-wrap items-center border-line px-3 py-2.5 md:px-5 md:py-0 xl:h-16 xl:flex-nowrap " +
        (hasMeta ? "xl:border-b" : "border-b")
      }>
      {onBack ? (
        <div className="mr-2 flex items-center md:h-16 lg:mr-0 xl:h-auto">
          <button
            type="button"
            onClick={onBack}
            aria-label="Back to conversations"
            className="inline-flex size-11 shrink-0 items-center justify-center rounded-control text-ink-2 hover:bg-subtle hover:text-ink lg:hidden">
            <ArrowLeftIcon aria-hidden="true" width={20} height={20} />
          </button>
        </div>
      ) : null}
      {/* Below 1280 this wrapper melts away: the title is a flex item and the meta line a full-width row after the pill and actions. */}
      <div className="contents xl:block xl:min-w-0 xl:flex-1">
        <div className="min-w-0 flex-1 md:flex md:h-16 md:flex-col md:justify-center xl:block xl:h-auto xl:flex-none">
          <h2 className="truncate font-display text-base font-semibold text-ink">{title}</h2>
          {/* (chat-11) On a phone the list is one tap back: the arrow and its word, "Conversations", under the title. */}
          {onBack ? <p className="text-xs text-muted md:hidden">Conversations</p> : null}
        </div>
        {hasMeta ? (
          // (FIX-235) Flex below xl (gap-separated items, full-width row). At xl the meta shares
          // the 64 px header with the title, so it flows as inline text instead: an atomic
          // visibility item would wrap to two rows of its own and push the segments to a third,
          // overflowing the header (agent-page-p5 pins its height). Whitespace-only text nodes
          // give the inline spacing; flex layout ignores them (the gap does that job below xl).
          <p
            data-testid="thread-meta"
            className="order-2 -mx-3 mt-2 flex grow basis-full flex-wrap items-center gap-x-1.5 gap-y-0.5 border-y border-line px-3 py-2 text-xs text-muted md:-mx-5 md:-mt-px md:px-5 xl:m-0 xl:block xl:w-auto xl:border-0 xl:p-0">
            {visibility ? (
              <span className="inline-flex min-w-0 items-start gap-1 xl:contents">
                {visibilityIcon === "people" ? (
                  // (FIX-235) At xl the meta is inline text; preflight makes svg a block, which
                  // would give the icon a line of its own, so it goes back to inline there.
                  <UsersIcon aria-hidden="true" width={12} height={12} className="mt-0.5 shrink-0 xl:inline" />
                ) : (
                  <LockIcon aria-hidden="true" width={12} height={12} className="mt-0.5 shrink-0 xl:inline" />
                )}
                {" "}
                <span>
                  {retention && xl ? (
                    // (retention-tooltip) The line itself carries the note: focus or tap it and the retention
                    // window shows as its tooltip (its accessible description), Escape or blur closes.
                    <InfoTip note={retention} testId="retention-tip">
                      {visibility}
                    </InfoTip>
                  ) : (
                    visibility
                  )}
                  {segments.length > 0 ? <Separator className={classOf(segments[0]!)} /> : null}
                </span>
              </span>
            ) : null}
            {segments.map((seg, i) => {
              const next = segments[i + 1];
              return (
                <Fragment key={textOf(seg)}>
                  {visibility || i > 0 ? " " : null}
                  <span className={classOf(seg)}>
                    {textOf(seg)}
                    {next !== undefined ? <Separator className={classOf(next)} /> : null}
                  </span>
                </Fragment>
              );
            })}
          </p>
        ) : null}
      </div>
      <div className="order-1 ml-2 flex items-center gap-2 md:h-16 xl:order-none xl:h-auto">
        {live === null ? null : live ? <Badge tone="accent">Live</Badge> : <Badge>Reconnecting…</Badge>}
        {actions}
      </div>
    </header>
  );
}
