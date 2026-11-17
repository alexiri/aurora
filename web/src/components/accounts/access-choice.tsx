"use client";

import { WhoCan } from "@/components/ui/who-can";
import { ChoiceCards } from "@/components/ui/choice-cards";
import type { Account, Level } from "@/lib/api/accounts";
import { isLimited, LEVEL_LABEL, LEVEL_ORDER, levelSentence } from "@/lib/accounts";

/**
 * The three choices as cards (`14 §2`), cut to the account's `levels`. Presentational: the row, the picker and the
 * journey each own what "choose" saves. A cut by an organization's limit says "Limited by your organization" with who
 * can change it; no choices at all renders nothing (websites, AI keys and read-only apps have none to make).
 */
export function AccessChoice({
  account,
  agentName,
  value,
  onChange,
  disabled = false,
  legend = "Access",
  orgName,
}: {
  account: Pick<Account, "levels" | "limit" | "app" | "managed_by">;
  agentName: string;
  /** `null` when the saved choice is not one of the three (custom): no card is chosen. */
  value: Level | null;
  onChange: (level: Level) => void;
  disabled?: boolean;
  legend?: string;
  /** The organization that sets the limit: "Limited by <org>". */
  orgName?: string;
}) {
  const levels = LEVEL_ORDER.filter((l) => account.levels.includes(l));
  if (levels.length === 0) return null;
  return (
    <div className="space-y-2">
      <ChoiceCards<Level | "none">
        legend={legend}
        legendHidden
        value={value ?? "none"}
        disabled={disabled}
        onChange={(v) => {
          if (v !== "none") onChange(v);
        }}
        options={levels.map((l) => ({
          value: l,
          label: LEVEL_LABEL[l],
          description: levelSentence(l, agentName),
          badge: l === "read" ? "Recommended" : undefined,
        }))}
      />
      {isLimited(account) ? (
        <div className="text-sm text-ink-2">
          <p>Limited by {orgName ?? "your organization"}.</p>
          {account.managed_by ? <WhoCan value={account.managed_by} /> : null}
        </div>
      ) : null}
    </div>
  );
}
