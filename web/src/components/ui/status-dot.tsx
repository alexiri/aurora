import { cva, type VariantProps } from "class-variance-authority";
import type { ReactNode } from "react";

import { cn } from "@/lib/utils";

const dotVariants = cva("inline-block size-2 shrink-0 rounded-full", {
  variants: {
    tone: {
      neutral: "bg-muted",
      accent: "bg-accent",
      pending: "bg-pending-ink",
      danger: "bg-danger-ink",
    },
  },
  defaultVariants: { tone: "neutral" },
});

/** A status dot with its text label (the dot alone never carries meaning). */
export function StatusDot({
  tone,
  children,
  className,
}: VariantProps<typeof dotVariants> & { children: ReactNode; className?: string }) {
  return (
    <span className={cn("inline-flex items-center gap-1.5 text-xs font-medium text-ink-2", className)}>
      <span aria-hidden="true" className={dotVariants({ tone })} />
      {children}
    </span>
  );
}
