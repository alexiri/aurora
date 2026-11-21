import { cn } from "@/lib/utils";

/** The "AI" tile (64 px, as an app icon, FIX-211) that stands for an AI key: account rows, the organization's keys on AI models. */
export function AiKeyTile({ className }: { className?: string }) {
  return (
    <span aria-hidden="true" className={cn("inline-flex size-16 shrink-0 items-center justify-center rounded-control border border-line bg-accent-soft text-lg font-semibold text-accent-ink", className)}>
      AI
    </span>
  );
}
