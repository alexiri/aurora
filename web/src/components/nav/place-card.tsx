import Link from "next/link";
import type { ReactNode } from "react";

import { ChevronLeftIcon } from "@/components/ui/icons";
import type { BackLink } from "@/lib/nav/manage-return";

/**
 * (D-508(a), `14` §Places) The place card at the top of the rail: the same box in every place, saying where you are.
 * It is the picker's button (`scope-switcher.tsx`); with nothing else to open it is a plain card.
 */
export const PLACE_CARD_CLASS = "flex min-h-12 w-full items-center gap-2.5 rounded-control border border-line-strong bg-surface px-2.5 py-1.5";

/** The two lines of a place card, after its mark. */
export function PlaceText({ name, kind }: { name: ReactNode; kind?: ReactNode }) {
  return (
    <span className="min-w-0 flex-1 text-left">
      <span className="block truncate text-sm font-semibold text-ink">{name}</span>{" "}
      {kind ?<span className="block truncate text-xs text-muted">{kind}</span> : null}
    </span>
  );
}

/** The back line above the card of every place but a workspace: to the workspace and page you came from. */
export function PlaceBack({ back }: { back: BackLink }) {
  return (
    <Link
      href={back.href}
      data-testid="place-back"
      className="flex min-h-9 items-center gap-2.5 rounded-control px-2.5 py-1.5 text-sm text-ink-2 hover:bg-subtle hover:text-ink"
    >
      <ChevronLeftIcon className="size-4 shrink-0 text-muted" />
      <span className="min-w-0 truncate">
        <span className="sr-only">Back to </span>
        {back.name}
      </span>
    </Link>
  );
}
