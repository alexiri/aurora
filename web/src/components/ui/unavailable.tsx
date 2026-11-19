import type { ReactNode } from "react";

import type { WhoCanValue } from "@/lib/who-can";

import { LockIcon } from "./icons";
import { WhoCan } from "./who-can";

/**
 * (P5-POLISH-2, D-392, E6) A feature this Aurora does not have: one neutral card instead of a "can't be turned on here"
 * notice over a disabled form. A lock, a title ("The web terminal isn't available on this Aurora"), one sentence on what
 * is missing, who can fix it (`whoCan`, or `fixer` text such as "An operator installs it") and a guide link. The
 * settings are rendered only when they can take effect, so none sit under this card.
 */
export function Unavailable({
  title,
  children,
  whoCan,
  fixer,
  guide,
}: {
  title: string;
  /** What is missing, one sentence. */
  children: ReactNode;
  whoCan?: WhoCanValue;
  fixer?: ReactNode;
  /** The guide link (an anchor the caller builds). */
  guide?: ReactNode;
}) {
  return (
    <section aria-label={title} className="flex items-start gap-3 rounded-card border border-line bg-surface p-4 sm:p-5">
      <span aria-hidden="true" className="flex size-9 shrink-0 items-center justify-center rounded-control bg-subtle text-ink-2">
        <LockIcon className="size-5" />
      </span>
      <div className="min-w-0 space-y-1">
        <h3 className="font-display text-base font-semibold text-ink">{title}</h3>
        <p className="text-sm text-ink-2">{children}</p>
        {whoCan ? <WhoCan value={whoCan} /> : fixer ? <p className="text-sm text-ink-2">{fixer}</p> : null}
        {guide ? <p className="text-sm">{guide}</p> : null}
      </div>
    </section>
  );
}
