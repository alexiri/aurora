import { Meter } from "@/components/ui/meter";
import { formatCount } from "@/lib/format-date";
import { meter } from "@/lib/api/memory";

/**
 * Accessible usage meter; turns red and announces when over the limit. `quiet` (P5-MEMORY-2) is one line under an
 * editor: a thin bar, then "476 of 100,000 characters" in muted text.
 */
export function SizeMeter({ label, used, max, quiet = false }: { label: string; used: number; max: number; quiet?: boolean }) {
  const m = meter(used, max);
  if (quiet) {
    return (
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
        <Meter
          role="meter"
          aria-label={label}
          aria-valuemin={0}
          aria-valuemax={max}
          aria-valuenow={Math.min(used, max)}
          ratio={m.ratio}
          tone={m.over ? "danger" : "accent"}
          className="h-1 w-36"
        />
        <p className={m.over ? "text-xs text-danger-ink" : "text-xs text-muted"} role={m.over ? "alert" : undefined}>
          {formatCount(used)} of {formatCount(max)} characters
          {m.over ? " (over the limit)" : ""}
        </p>
      </div>
    );
  }
  return (
    <div className="space-y-1">
      <Meter
        role="meter"
        aria-label={label}
        aria-valuemin={0}
        aria-valuemax={max}
        aria-valuenow={Math.min(used, max)}
        ratio={m.ratio}
        tone={m.over ? "danger" : "accent"}
        className="h-1.5"
      />
      <p className={m.over ? "text-xs text-danger-ink" : "text-xs text-muted"} role={m.over ? "alert" : undefined}>
        {label}: {formatCount(used)} / {formatCount(max)} characters
        {m.over ? " (over the limit)" : ""}
      </p>
    </div>
  );
}
