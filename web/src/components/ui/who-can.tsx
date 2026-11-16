import Link from "next/link";
import type { ReactNode } from "react";

import { actionLabel, askSentence, onlySentence, type WhoCanValue } from "@/lib/who-can";
import { cn } from "@/lib/utils";

/**
 * (C-981, D-349) The one way to say who can fix something, fed by the server's `WhoCan` (refusals, blocked connect
 * methods, ladder rows) or by a workspace's or organization's `contacts` (`whoCanFromContacts`).
 *
 * - The viewer can do it themselves (`you_can` with a `path`): the link, "Add an account"; never a sentence about asking.
 * - `lead`: the full sentence for an empty state or a page header, "Only workspace owners can add an account. Ask Bob
 *   Builder."
 * - Otherwise just who to ask, with the role in brackets: "Ask Ada Lovelace or Alan Turing (organization owners and
 *   admins)." Instance admins are never named: "An Aurora administrator can set this up."
 */
export function WhoCan({
  value,
  lead = false,
  className,
  children,
}: {
  value: WhoCanValue;
  lead?: boolean;
  className?: string;
  /** Replaces the link text when the viewer can act. */
  children?: ReactNode;
}) {
  if (value.you_can && value.path) {
    return (
      <p className={cn("text-sm text-ink-2", className)}>
        <Link href={value.path} className="font-medium text-link underline">
          {children ?? actionLabel(value.action)}
        </Link>
      </p>
    );
  }
  const text = lead
    ? value.role === "instance_admin"
      ? onlySentence(value)
      : `${onlySentence(value)}${value.people.length > 0 ? ` ${askSentence(value, false)}` : ""}`
    : askSentence(value);
  return <p className={cn("text-sm text-ink-2", className)}>{text}</p>;
}
