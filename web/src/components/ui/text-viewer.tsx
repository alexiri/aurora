import { cn } from "@/lib/utils";

/**
 * Third-party text shown exactly as written: a labelled, keyboard-focusable preformatted region with a height
 * cap. Long lines scroll inside the region, so the page never scrolls sideways. The text is a React child, never
 * markup: no Markdown, no links, no HTML (SEC-117).
 */
export function TextViewer({ label, text, className }: { label: string; text: string; className?: string }) {
  return (
    <pre
      role="region"
      aria-label={label}
      tabIndex={0}
      className={cn(
        "max-h-96 min-w-0 max-w-full overflow-auto whitespace-pre rounded-control border border-line bg-subtle p-3 font-mono text-xs text-ink",
        className,
      )}
    >
      {text}
    </pre>
  );
}
