"use client";

import { CopyBlock } from "@/components/account/ssh-copy";
import { Altitude } from "@/components/ui/altitude";

export interface TerminalCommand {
  /** What the block is called to a screen reader ("the aurora ssh command"). */
  label: string;
  text: string;
  /** One line that says what it does. */
  note: string;
}

/**
 * (P5-REFINE-19, `fold.cli`, Expert) **From your terminal**: a closed `<details>` of copyable commands, each with its
 * one-line explanation. Only commands that exist in the CLI today (`services/aurora-cli`, `aurora <command> --help`) or
 * calls with placeholders for every secret; the callers list where each was confirmed in the brief's report.
 */
export function FromYourTerminal({ commands, title = "From your terminal" }: { commands: TerminalCommand[]; title?: string }) {
  if (commands.length === 0) return null;
  return (
    <Altitude id="fold.cli" level="expert">
      <details className="group rounded-card border border-line bg-surface" data-testid="from-your-terminal">
        <summary className="flex min-h-11 cursor-pointer list-none items-center justify-between gap-2 rounded-card px-4 py-2 text-sm font-medium text-ink hover:bg-subtle md:min-h-9 [&::-webkit-details-marker]:hidden">
          <span>{title}</span>
          <span aria-hidden="true" className="text-muted transition-transform group-open:rotate-90">
            ›
          </span>
        </summary>
        <div className="space-y-4 border-t border-line px-4 py-3">
          {commands.map((c) => (
            <div key={c.text} className="space-y-1">
              <CopyBlock text={c.text} label={c.label} />
              <p className="text-xs text-muted">{c.note}</p>
            </div>
          ))}
        </div>
      </details>
    </Altitude>
  );
}
