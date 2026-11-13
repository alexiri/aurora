import { cva, type VariantProps } from "class-variance-authority";
import type { HTMLAttributes } from "react";

import { cn } from "@/lib/utils";

const badgeVariants = cva(
  "inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-label font-semibold uppercase leading-4 tracking-label",
  {
    variants: {
      tone: {
        neutral: "bg-subtle text-ink-2",
        accent: "bg-accent-soft text-accent-ink",
        pending: "bg-pending-soft text-pending-ink",
        danger: "bg-danger-soft text-danger-ink",
      },
    },
    defaultVariants: { tone: "neutral" },
  },
);

/** A pill that always carries text, so meaning never depends on colour alone. */
export function Badge({
  tone,
  className,
  ...props
}: HTMLAttributes<HTMLSpanElement> & VariantProps<typeof badgeVariants>) {
  return <span className={cn(badgeVariants({ tone }), className)} {...props} />;
}

export { badgeVariants };
