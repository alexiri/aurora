import Link from "next/link";

import { askSentence, onlySentence, type WhoCanValue } from "@/lib/who-can";
import { cn } from "@/lib/utils";

import { LockIcon } from "./icons";

/**
 * (P5-POLISH-2, D-384) Who owns a value that the viewer cannot change here: a neutral pill with a lock, in sentence
 * case ("Set for you by an Aurora administrator", "Set by the installation", "Set for you by your organization's admins"). It is never amber: amber means "needs you".
 * `whoCan` is the tooltip and the accessible description (who can change it); `href` opens where it is changed, when
 * the viewer may go there.
 */
export function LockPill({
  owner,
  whoCan,
  href,
  className,
}: {
  owner: string;
  whoCan?: WhoCanValue;
  href?: string;
  className?: string;
}) {
  const who = whoCan ? `${onlySentence(whoCan)} ${askSentence(whoCan)}`.trim() : undefined;
  const cls = cn(
    "inline-flex items-center gap-1 rounded-full bg-subtle px-2 py-0.5 text-label font-medium leading-4 text-ink-2",
    href && "hover:text-ink hover:underline",
    className,
  );
  const body = (
    <>
      <LockIcon className="size-3" aria-hidden="true" />
      {owner}
      {who ? <span className="sr-only">. {who}</span> : null}
    </>
  );
  return href ? (
    <Link href={href} title={who} className={cls}>
      {body}
    </Link>
  ) : (
    <span title={who} className={cls}>
      {body}
    </span>
  );
}
