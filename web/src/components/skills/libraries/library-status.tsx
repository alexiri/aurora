import Link from "next/link";

import { AttentionChip } from "@/components/ui/attention-chip";
import { Badge } from "@/components/ui/badge";
import { LIBRARY_STATUS, label } from "@/lib/labels";
import { awaitsCheck, type SkillLibrary } from "@/lib/api/skill-libraries";

/** A library migrated without an address has nothing to read until someone enters it (C-1127). */
export function needsAddress(l: SkillLibrary): boolean {
  return l.status === "error" && l.url === null;
}

/** The reader's sanitized reason says the repository wants a token (C-1125: "The repository needs an access token"). */
export function needsToken(l: SkillLibrary): boolean {
  return l.status === "error" && /needs an access token/i.test(l.last_error ?? "");
}

/** Why a library "Needs attention", in words; the reason is repository text: data, never markup. */
export function libraryReason(l: SkillLibrary): string | null {
  if (needsAddress(l)) return "Enter the repository's git address";
  if (needsToken(l)) return "The repository needs an access token";
  return l.status === "error" ? (l.last_error ?? null) : null;
}

/**
 * The one state chip of a library card (D-506, skill-6): Up to date, Update available, Paused, Needs attention or
 * Over budget. Something the viewer can act on is amber (`actionId` is the element on the card that acts on it); a
 * library the viewer only reads is never amber. `reviewHref` is where the waiting updates are reviewed, and
 * `reviewable` is how many of them that page shows: `pending_updates` counts every level, and with none there the chip
 * links nowhere (the card says where the rest wait).
 */
export function LibraryStateChip({
  library: l,
  actionId,
  reviewHref,
  reviewable,
}: {
  library: SkillLibrary;
  actionId?: string;
  reviewHref?: string;
  reviewable: number;
}) {
  const pending = l.pending_updates ?? 0;
  const attention = (text: string) => (actionId ? <AttentionChip actionId={actionId}>{text}</AttentionChip> : <Badge>{text}</Badge>);
  if (l.status === "paused") return <Badge>{label(LIBRARY_STATUS, "paused")}</Badge>;
  if (l.status === "error" || l.status === "over_budget") return attention(label(LIBRARY_STATUS, l.status));
  if (pending > 0) {
    if (!reviewHref || !actionId || reviewable === 0) return <Badge>Update available</Badge>;
    return (
      <Link
        href={reviewHref}
        id={actionId}
        aria-label={`Update available: ${reviewable} pending ${reviewable === 1 ? "update" : "updates"} of ${l.label}, review ${reviewable === 1 ? "it" : "them"}`}
        className="inline-flex min-h-6 items-center rounded-full focus-visible:outline focus-visible:outline-2 focus-visible:outline-accent"
      >
        <AttentionChip actionId={actionId}>Update available</AttentionChip>
      </Link>
    );
  }
  // Nothing is compared yet, so "Up to date" would be a guess.
  if (awaitsCheck(l)) return <Badge>Check pending</Badge>;
  return <Badge tone="accent">{label(LIBRARY_STATUS, "active")}</Badge>;
}
