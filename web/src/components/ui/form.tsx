import type { FormHTMLAttributes, ReactNode } from "react";

import { cn } from "@/lib/utils";

/** A group of related fields with a heading and help text (`<fieldset>`). Fields go inside, each a `Field`. */
export function FormSection({
  title,
  description,
  children,
  className,
}: {
  title: string;
  description?: ReactNode;
  children: ReactNode;
  className?: string;
}) {
  return (
    <fieldset className={cn("min-w-0 space-y-4 rounded-card border border-line bg-surface p-4 sm:p-5", className)}>
      <legend className="px-1 font-display text-base font-semibold text-ink">{title}</legend>
      {description ? <p className="-mt-2 text-sm text-ink-2">{description}</p> : null}
      {children}
    </fieldset>
  );
}

/** Form with vertical rhythm; max width keeps lines readable. */
export function Form({ className, ...props }: FormHTMLAttributes<HTMLFormElement>) {
  return <form className={cn("max-w-xl space-y-5", className)} {...props} />;
}

/** Submit row: primary action first in the DOM, wraps on narrow screens. Status text goes in `children`. */
export function FormActions({ children, className }: { children: ReactNode; className?: string }) {
  return <div className={cn("flex flex-wrap items-center gap-3", className)}>{children}</div>;
}
