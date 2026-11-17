"use client";

import { useState } from "react";

import { ErrorState } from "@/components/states";
import { Button } from "@/components/ui/button";
import { type Account, type AgentAccess, usePutAccess } from "@/lib/api/accounts";
import { useGrants } from "@/lib/api/policy";
import { LEVEL_LABEL } from "@/lib/accounts";

import { AccessChoice } from "./access-choice";
import { ActionsTable } from "./actions-table";

/**
 * One agent's access to one app account (C-938, `14 §2`): the three choices from the account's `levels`, the changes
 * allowed without asking, "Custom: <n> actions differ from <level>", and **Custom…**, which opens the actions table.
 * One save applies a choice (C-980). People who may not change the access see it as text, no controls.
 *
 * Accounts with no choice (websites, AI keys, read-only apps) render nothing: the agent can use them or not.
 */
export function AccessControl({
  account,
  entry,
  workspaceId,
  canChange,
  startOpen = false,
}: {
  account: Account;
  entry: AgentAccess;
  workspaceId: string;
  canChange: boolean;
  /** A `custom_only` app opens the table directly (C-410). */
  startOpen?: boolean;
}) {
  const put = usePutAccess();
  const grants = useGrants(entry.agent.id);
  const [table, setTable] = useState(startOpen);
  if (account.levels.length === 0 || account.ref.type !== "connection") return null;
  const level = entry.level === "read" || entry.level === "ask" || entry.level === "act" ? entry.level : null;
  const always = entry.always_allowed ?? [];

  const askAgain = (toolId: string) => {
    const saved = grants.data?.connections.find((c) => c.connection_id === account.ref.id);
    if (!saved) return;
    put.mutate({
      agentId: entry.agent.id,
      connectionId: account.ref.id,
      body: {
        mode: "custom",
        allow_tools: saved.allow_tools.filter((t) => t !== toolId),
        ask_tools: [...saved.ask_tools, toolId],
        ...(saved.ask_timeout_secs != null ? { ask_timeout_secs: saved.ask_timeout_secs } : {}),
      },
    });
  };

  const alwaysList =
    level === "ask" && always.length > 0 ? (
      <div className="text-sm text-ink-2">
        <p>
          <span className="font-medium text-ink">Always allowed:</span> {always.map((a) => a.name).join(", ")}
        </p>
        {canChange ? (
          <ul className="mt-1 flex flex-wrap gap-2" aria-label="Always allowed changes">
            {always.map((a) => (
              <li key={a.tool_id}>
                <Button variant="outline" size="sm" disabled={put.isPending} onClick={() => askAgain(a.tool_id)}>
                  Ask again for {a.name}
                </Button>
              </li>
            ))}
          </ul>
        ) : null}
      </div>
    ) : null;

  const customNote =
    entry.level === "custom" && entry.custom ? (
      <p className="text-sm text-ink-2">
        <span className="font-medium text-ink">Custom:</span> {entry.custom.differs} {entry.custom.differs === 1 ? "action differs" : "actions differ"} from{" "}
        {LEVEL_LABEL[entry.custom.base]}
      </p>
    ) : null;

  if (!canChange) {
    return (
      <div className="space-y-1 text-sm text-ink-2">
        {level ? <p>{LEVEL_LABEL[level]}</p> : null}
        {alwaysList}
        {customNote}
      </div>
    );
  }

  return (
    <div className="w-full space-y-3">
      <AccessChoice account={account} agentName={entry.agent.name} value={level} disabled={put.isPending} onChange={(l) => put.mutate({ agentId: entry.agent.id, connectionId: account.ref.id, body: { level: l } })} />
      {alwaysList}
      {customNote}
      {put.isError ? <ErrorState error={put.error} /> : null}
      <Button variant="outline" size="sm" onClick={() => setTable(true)}>
        Custom…
      </Button>
      <ActionsTable
        account={account}
        entry={entry}
        agentId={entry.agent.id}
        agentName={entry.agent.name}
        workspaceId={workspaceId}
        open={table}
        onOpenChange={setTable}
      />
    </div>
  );
}
