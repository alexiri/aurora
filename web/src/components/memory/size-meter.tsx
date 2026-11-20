import { Meter } from "@/components/ui/meter";
import { formatCount } from "@/lib/format-date";
import { meter } from "@/lib/api/memory";

/** Accessible usage meter; turns red and announces when over the limit. */
export function SizeMeter({ label, used, max }: { label: string; used: number; max: number }) {
  const m = meter(used, max);
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
