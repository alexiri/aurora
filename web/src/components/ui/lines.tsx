"use client";

import { diffLines } from "@/lib/api/skill-libraries";

/**
 * The one diff primitive (the library update review's, shared with the template review): every
 * line of old and new, marked `+`/`-` and tinted — the whole text, nothing summarized or truncated.
 */
export function Lines({ old, next, label }: { old: string; next: string; label: string }) {
  const lines = diffLines(old, next);
  return (
    <pre tabIndex={0} aria-label={label} className="max-w-full overflow-x-auto rounded-control border border-line bg-canvas p-2 font-mono text-xs leading-5 text-ink">
      {lines.map((l, i) => (
        <span
          key={i}
          data-diff={l.kind}
          className={`block whitespace-pre ${l.kind === "add" ? "bg-accent-soft text-accent-ink" : l.kind === "del" ? "bg-danger-soft text-danger-ink" : ""}`}
        >
          {l.kind === "add" ? "+ " : l.kind === "del" ? "- " : "  "}
          {l.text}
        </span>
      ))}
    </pre>
  );
}
