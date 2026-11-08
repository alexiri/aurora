import { meter } from "@/lib/api/memory";

/** Accessible usage meter; turns red and announces when over the limit. */
export function SizeMeter({ label, used, max }: { label: string; used: number; max: number }) {
  const m = meter(used, max);
  return (
    <div className="space-y-1">
      <div
        role="meter"
        aria-label={label}
        aria-valuemin={0}
        aria-valuemax={max}
        aria-valuenow={Math.min(used, max)}
        className="h-1.5 w-full overflow-hidden rounded-full bg-subtle"
      >
        <div className={m.over ? "h-full bg-danger-ink" : "h-full bg-accent"} style={{ width: `${m.ratio * 100}%` }} />
      </div>
      <p className={m.over ? "text-xs text-danger-ink" : "text-xs text-muted"} role={m.over ? "alert" : undefined}>
        {label}: {used.toLocaleString("en-US")} / {max.toLocaleString("en-US")} characters
        {m.over ? " (over the limit)" : ""}
      </p>
    </div>
  );
}
