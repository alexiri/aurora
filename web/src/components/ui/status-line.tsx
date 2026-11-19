import type { ReactNode } from "react";

import { cn } from "@/lib/utils";

export type StatusTone = "ok" | "fail" | "idle" | "attention";

const DOT: Record<StatusTone, string> = {
  ok: "bg-accent",
  fail: "bg-danger-ink",
  idle: "bg-muted",
  attention: "bg-pending-ink",
};
const TEXT: Record<StatusTone, string> = {
  ok: "text-ink-2",
  fail: "text-danger-ink",
  idle: "text-muted",
  attention: "text-pending-ink",
};

/**
 * (P5-POLISH-2, D-383, F2) A dot and one sentence-case line, with an optional trailing action ("Reconnect", "Sign in
 * again"). Purely visual: the tone only colours the dot and the text, the sentence carries the meaning. An
 * `attention` line should come with its `action`.
 */
export function StatusLine({
  tone,
  children,
  action,
  className,
  "data-testid": testId,
}: {
  "data-testid"?: string;
  tone: StatusTone;
  children: ReactNode;
  action?: ReactNode;
  className?: string;
}) {
  return (
    <p data-testid={testId} className={cn("flex flex-wrap items-center gap-x-2 gap-y-1 text-sm", TEXT[tone], className)}>
      <span aria-hidden="true" className={cn("size-2 shrink-0 rounded-full", DOT[tone])} />
      <span className="min-w-0 flex-1">{children}</span>
      {action}
    </p>
  );
}
