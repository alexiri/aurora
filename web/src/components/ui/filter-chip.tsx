import { XIcon } from "@/components/ui/icons";
import { cn } from "@/lib/utils";

/** An active filter: its text and a remove button (a 44 px target on phones). */
export function FilterChip({ label, onRemove, className }: { label: string; onRemove: () => void; className?: string }) {
  return (
    <span className={cn("inline-flex max-w-full items-center gap-1 rounded-full bg-subtle py-0.5 pl-2.5 pr-0.5 text-xs font-medium text-ink-2", className)}>
      <span className="truncate">{label}</span>
      <button
        type="button"
        onClick={onRemove}
        aria-label={`Remove filter ${label}`}
        className="inline-flex size-11 shrink-0 items-center justify-center rounded-full text-muted hover:text-ink md:size-6">
        <XIcon aria-hidden="true" width={12} height={12} />
      </button>
    </span>
  );
}
