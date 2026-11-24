import { cn } from "@/lib/utils";

/**
 * The steps of a multi-step page or dialog (P6-TPL-4 B1): one horizontal list of labels over a top rule; the finished
 * steps and the current one are accented, the ones still ahead are muted; the current one carries `aria-current`.
 * Presentational — the host owns which step is current and what each step renders.
 */
export function Stepper({
  steps,
  current,
  label = "Steps",
  className,
}: {
  steps: { id: string; label: string }[];
  /** The id of the current step. */
  current: string;
  label?: string;
  className?: string;
}) {
  const at = steps.findIndex((s) => s.id === current);
  return (
    <ol className={cn("flex gap-1.5 text-xs font-semibold uppercase tracking-wider", className)} aria-label={label}>
      {steps.map((s, i) => (
        <li
          key={s.id}
          aria-current={s.id === current ? "step" : undefined}
          className={cn("flex-1 border-t-4 pt-1.5", i < at ? "border-accent text-accent-ink" : s.id === current ? "border-accent text-ink" : "border-line text-muted")}
        >
          {s.label}
        </li>
      ))}
    </ol>
  );
}
