import { cn } from "@/lib/utils";

/** The "AI" tile that stands for an AI key: account rows, the organization's keys on AI models. */
export function AiKeyTile({ className }: { className?: string }) {
  return (
    <span aria-hidden="true" className={cn("inline-flex size-8 shrink-0 items-center justify-center rounded-control border border-line bg-accent-soft text-xs font-semibold text-accent-ink", className)}>
      AI
    </span>
  );
}
