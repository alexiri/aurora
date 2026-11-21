import type { ReactNode } from "react";

import { cn } from "@/lib/utils";

/**
 * The grid every list of cards uses (D-439, docs/14 §Visual design): Apps, Accounts, Agents. One column below `sm`,
 * two from `sm`, three from `lg`, four from `2xl`. Its children are `<li>` cards; a card fills its cell (`h-full`)
 * and puts its actions at the foot (`mt-auto`).
 */
export function CardGrid({
  children,
  className,
  ...rest
}: {
  children: ReactNode;
  className?: string;
  "aria-label"?: string;
  "aria-hidden"?: boolean | "true" | "false";
}) {
  return (
    <ul {...rest} className={cn("grid gap-4 sm:grid-cols-2 lg:grid-cols-3 2xl:grid-cols-4", className)}>
      {children}
    </ul>
  );
}
