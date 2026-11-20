"use client";

import { Button } from "@/components/ui/button";
import { useCopy } from "@/lib/use-copy";
import { cn } from "@/lib/utils";

/** Copy `text` to the clipboard; the button says "Copied" for two seconds. A browser that refuses says so. */
export function CopyButton({ text, label, className }: { text: string; label: string; className?: string }) {
  const { state, copy } = useCopy();
  return (
    <>
      <Button
        type="button"
        variant="outline"
        size="sm"
        className={className}
        aria-label={`Copy ${label}`}
        onClick={() => copy(text)}
      >
        Copy
      </Button>
      {state !== "idle" ? (
        <span role="status" className={cn("text-xs", state === "copied" ? "text-accent-ink" : "text-danger-ink")}>
          {state === "copied" ? "Copied" : "Your browser blocked copying; select the text instead."}
        </span>
      ) : null}
    </>
  );
}

/**
 * A command or a few lines, monospace, with a Copy button. Long lines wrap inside the block (never scroll the page, 390 px).
 */
export function CopyBlock({ text, label, display }: { text: string; label: string; /** What the block shows when it differs from what Copy puts on the clipboard (an id kept out of sight). */ display?: string }) {
  return (
    <div className="space-y-1.5">
      <pre
        tabIndex={0}
        aria-label={label}
        className="max-w-full overflow-x-auto whitespace-pre-wrap break-all rounded-control border border-line bg-subtle p-3 font-mono text-xs text-ink"
      >
        {display ?? text}
      </pre>
      <div className="flex flex-wrap items-center gap-2">
        <CopyButton text={text} label={display ? "command" : label} />
      </div>
    </div>
  );
}

/** `SHA256:abcdef…uvwxyz`: long fingerprints keep their head and tail (the whole value is in the tooltip). */
export function middleTruncate(s: string, head = 14, tail = 8): string {
  return s.length <= head + tail + 1 ? s : `${s.slice(0, head)}…${s.slice(-tail)}`;
}
