"use client";

import { Button } from "@/components/ui/button";
import type { Account } from "@/lib/api/accounts";
import { relativeTime } from "@/lib/api/files";
import { healthLine, testOutcomeText, type HealthFix } from "@/lib/account-health";

/**
 * The health line of an account row (`14 §7`, C-945): one wording per state, from `lib/account-health`, with the fix
 * beside it. `outcome` replaces the line after a Test that stored nothing (an account that cannot be tested, or a test
 * that could not run). `onFix` is given for the fixes the viewer may make; without it the line is text only.
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
  if (outcome) return <p className="text-sm text-muted">{testOutcomeText(outcome)}</p>;
  const used = account.health.last_used_at ? relativeTime(account.health.last_used_at) : undefined;
  const line = healthLine(account, used);
  return (
    <p className={`flex flex-wrap items-center gap-x-2 text-sm ${line.tone === "attention" ? "text-danger-ink" : "text-muted"}`}>
      <span>{line.text}</span>
      {line.fix && onFix ? (
        <Button type="button" variant="outline" size="sm" onClick={() => onFix(line.fix!.kind)}>
          {line.fix.label}
        </Button>
      ) : null}
    </p>
  );
}
