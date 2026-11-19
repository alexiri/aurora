import { cva, type VariantProps } from "class-variance-authority";
import type { HTMLAttributes } from "react";

import { errorMessage } from "@/lib/api/errors";
import { cn } from "@/lib/utils";

const noticeVariants = cva("rounded-control border p-3 text-sm", {
  variants: {
    tone: {
      neutral: "border-line bg-subtle text-ink-2",
      accent: "border-accent/30 bg-accent-soft text-accent-ink",
      pending: "border-pending-line bg-pending-panel text-pending-ink",
      danger: "border-danger-ink/30 bg-danger-soft text-danger-ink",
    },
  },
  defaultVariants: { tone: "neutral" },
});

/**
 * Inline callout for conflicts, warnings, errors and info. Pass `role="alert"` for errors that
 * appear after an action and `role="status"` for passive information.
 */
export function Notice({ tone, className, ...props }: HTMLAttributes<HTMLDivElement> & VariantProps<typeof noticeVariants>) {
  return <div className={cn(noticeVariants({ tone }), className)} {...props} />;
}

/**
 * The danger `Notice` for a failed request or mutation: the error's own words (`errorMessage`, or
 * `format` where a screen has its own code table), announced as an alert. Renders nothing without an error.
 */
export function ErrorNotice({
  error,
  format = errorMessage,
  className,
}: {
  error: unknown;
  format?: (error: unknown) => string;
  className?: string;
}) {
  if (!error) return null;
  return (
    <Notice tone="danger" role="alert" className={className}>
      {format(error)}
    </Notice>
  );
}
