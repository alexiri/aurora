"use client";

import { Button } from "@/components/ui/button";
import { StatusLine, type StatusTone } from "@/components/ui/status-line";
import type { Account } from "@/lib/api/accounts";
import { healthLine, testOutcomeText, type HealthFix } from "@/lib/account-health";

const TONE: Record<"ok" | "attention" | "neutral", StatusTone> = { ok: "ok", attention: "fail", neutral: "idle" };

/**
 * The health line of an account row (`14 §7`, C-945, D-383): one wording per state, from `lib/account-health`, drawn as
 * a `StatusLine` with the fix beside it. `outcome` replaces the line after a Test that stored nothing (an account that
 * cannot be tested, or a test that could not run). `onFix` is given for the fixes the viewer may make; without it the
 * line is text only. A stored test result shows apart ("Test passed 5 min ago"), never as the account working.
 */
export function AccountHealth({
  account,
  outcome,
  onFix,
}: {
  account: Pick<Account, "health" | "app" | "name" | "status_reason">;
  outcome?: "not_testable" | "error" | null;
  onFix?: (kind: HealthFix) => void;
}) {
  if (outcome) return <StatusLine tone="idle">{testOutcomeText(outcome)}</StatusLine>;
  const line = healthLine(account);
  return (
    <div className="space-y-0.5">
      <StatusLine
        tone={TONE[line.tone]}
        action={
          line.fix && onFix ? (
            // py-1: in a narrow card column the label takes two lines.
            <Button type="button" variant="outline" size="sm" className="py-1" onClick={() => onFix(line.fix!.kind)}>
              {line.fix.label}
            </Button>
          ) : undefined
        }
      >
        {line.text}
      </StatusLine>
      {line.test ? <p className="pl-4 text-xs text-muted">{line.test.text}</p> : null}
    </div>
  );
}
