import { Button } from "./button";
import { XIcon } from "./icons";
import { cn } from "@/lib/utils";

export type UploadTrayState = "queued" | "uploading" | "waiting" | "done" | "failed" | "cancelled" | "skipped";

export interface UploadTrayItem {
  id: string;
  /** What is being uploaded (a name or a relative path); rendered as text. */
  label: string;
  state: UploadTrayState;
  /** 0..1 while `uploading`. */
  progress?: number;
  /** The reason a `failed` item failed, in words. */
  error?: string;
  /** Extra text after the label (for example "kept as report (1).txt"). */
  note?: string;
}

const STATE_TEXT: Record<Exclude<UploadTrayState, "failed">, string> = {
  queued: "Waiting",
  uploading: "Uploading",
  waiting: "Needs your answer",
  done: "Done",
  cancelled: "Cancelled",
  skipped: "Skipped",
};

/**
 * The list of uploads in flight and just finished (P5-FILES-2, docs/14 §Files), under a toolbar: the chat attachment chip
 * states — progress with Cancel, done, failed as a `danger` chip with the reason. Presentational: the owner runs the queue.
 * Failed and finished items stay until dismissed, so a failure is never lost.
 */
export function UploadTray({
  label = "Uploads",
  items,
  onCancel,
  onDismiss,
  onClear,
}: {
  label?: string;
  items: UploadTrayItem[];
  onCancel: (id: string) => void;
  onDismiss: (id: string) => void;
  /** Removes every finished item. */
  onClear?: () => void;
}) {
  if (items.length === 0) return null;
  // (P5-REFINE-15, computer-10) Everything done: the panel collapses to one line and stops pushing the list below the fold.
  if (items.every((i) => i.state === "done")) {
    return (
      <section aria-label={label} className="flex flex-wrap items-center gap-x-2 text-sm text-ink-2" data-testid="uploads-done">
        <span>
          {label}: {items.length} {items.length === 1 ? "file" : "files"} done
        </span>
        <button type="button" className="min-h-9 text-accent-ink underline underline-offset-2 hover:no-underline" onClick={() => (onClear ? onClear() : items.forEach((i) => onDismiss(i.id)))}>
          Hide
        </button>
      </section>
    );
  }
  const finished = items.filter((i) => i.state === "done" || i.state === "failed" || i.state === "cancelled" || i.state === "skipped").length;
  return (
    <section aria-label={label} className="space-y-1.5 rounded-card border border-line bg-surface p-3">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h2 className="text-sm font-semibold text-ink">
          {label} <span className="font-normal text-muted">({items.length - finished} active)</span>
        </h2>
        {onClear && finished > 0 ? (
          <Button type="button" variant="ghost" size="sm" onClick={onClear}>
            Clear finished
          </Button>
        ) : null}
      </div>
      <ul className="space-y-1.5">
        {items.map((item) => (
          <li
            key={item.id}
            data-state={item.state}
            className={cn(
              "flex min-h-9 flex-wrap items-center gap-x-2 gap-y-1 rounded-control border px-2.5 py-1 text-xs",
              item.state === "failed" ? "border-danger-ink/30 bg-danger-soft text-danger-ink" : "border-line-strong bg-surface text-ink-2 dark:bg-canvas",
            )}>
            <span className="min-w-0 flex-1 break-all text-ink" title={item.label}>
              {item.label}
            </span>
            {item.state === "uploading" ? (
              <progress aria-label={`Uploading ${item.label}`} className="h-1.5 w-24 shrink-0 accent-accent" max={1} value={item.progress ?? 0} />
            ) : null}
            <span role={item.state === "failed" ? "alert" : "status"} className={cn("min-w-0 break-words", item.state === "failed" ? "" : "text-muted")}>
              {item.state === "failed" ? item.error || "Upload failed." : STATE_TEXT[item.state]}
              {item.note ? ` · ${item.note}` : ""}
            </span>
            {item.state === "queued" || item.state === "uploading" || item.state === "waiting" ? (
              <button
                type="button"
                onClick={() => onCancel(item.id)}
                aria-label={`Cancel ${item.label}`}
                className="inline-flex size-11 shrink-0 items-center justify-center rounded-control text-ink hover:bg-subtle hover:text-danger-ink md:size-8">
                <XIcon width={14} height={14} />
              </button>
            ) : (
              <button
                type="button"
                onClick={() => onDismiss(item.id)}
                aria-label={`Dismiss ${item.label}`}
                className="inline-flex size-11 shrink-0 items-center justify-center rounded-control text-ink hover:bg-subtle md:size-8">
                <XIcon width={14} height={14} />
              </button>
            )}
          </li>
        ))}
      </ul>
    </section>
  );
}
