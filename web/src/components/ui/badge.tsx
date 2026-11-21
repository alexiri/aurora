import { cva, type VariantProps } from "class-variance-authority";
import type { HTMLAttributes } from "react";

import { cn } from "@/lib/utils";

const badgeVariants = cva(
  "inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-label font-medium leading-4",
  {
    variants: {
      tone: {
        neutral: "bg-subtle text-ink-2",
        accent: "bg-accent-soft text-accent-ink",
        danger: "bg-danger-soft text-danger-ink",
        // The amber state chip with no action beside it (the list row of a conversation whose schedule is gone, schedules-7).
        pending: "bg-pending-soft text-pending-ink",
      },
    },
    defaultVariants: { tone: "neutral" },
  },
);

/**
 * A pill that always carries text, so meaning never depends on colour alone. Sentence case in every tone. There is no
 * amber tone for a need: amber is `AttentionChip` (something needs the viewer, with an action beside it); ownership is
 * `LockPill`. `pending` is only the amber of a settled state that is worth a look and has no action ("Schedule deleted").
 */
export function Badge({
  tone,
  className,
  ...props
}: HTMLAttributes<HTMLSpanElement> & VariantProps<typeof badgeVariants>) {
  return <span className={cn(badgeVariants({ tone }), className)} {...props} />;
}

export { badgeVariants };
