"use client";

import { useState } from "react";

import { ErrorState } from "@/components/states";
import { Button } from "@/components/ui/button";
import { InlineSaved, useSaveBar } from "@/components/ui/save-bar";
import { WhoCan } from "@/components/ui/who-can";
import { type Account, type AgentAccess, type Level, usePutAccess } from "@/lib/api/accounts";
import { useGrants } from "@/lib/api/policy";
import { cn } from "@/lib/utils";
import { isLimited, LEVEL_LABEL, LEVEL_ORDER } from "@/lib/accounts";

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
  orgName,
  inRow = false,
}: {
  account: Account;
  entry: AgentAccess;
  workspaceId: string;
  canChange: boolean;
  /** A `custom_only` app opens the table directly (C-410). */
  startOpen?: boolean;
  /** The organization that limits the account, for "Limited by <org>". */
  orgName?: string;
  /** In an account row's header: the level is its own item before the menu, the rest wraps below (P5-POLISH-11, C1). */
  inRow?: boolean;
}) {
  const put = usePutAccess();
  const saved = useSaveBar();
  const grants = useGrants(entry.agent.id);
  const [table, setTable] = useState(startOpen);
  if (account.ref.type !== "connection") return null;
  // (P5-POLISH-11) An app with no write tools has no choice, but the row still says what the agent may do.
  if (account.levels.length === 0) return <p className={cn("text-sm text-ink", inRow && "whitespace-nowrap")}>Read only</p>;
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

  // One table, from **What exactly?** or **Custom…**, inside the row (C-934, C-938).
  const details = (
    <>
      {table ? (
        <ActionsTable
          account={account}
          entry={entry}
          agentId={entry.agent.id}
          agentName={entry.agent.name}
          workspaceId={workspaceId}
          canChange={canChange}
          onClose={() => setTable(false)}
        />
      ) : null}
    </>
  );
  const exactly = (
    <Button variant="outline" size="sm" aria-expanded={table} onClick={() => setTable((v) => !v)}>
      What exactly?
    </Button>
  );

  if (!canChange) {
    if (inRow)
      return (
        <>
          {level ? <p className="whitespace-nowrap text-sm text-ink">{LEVEL_LABEL[level]}</p> : null}
          <div className="order-last w-full basis-full space-y-2 text-sm text-ink-2">
            {alwaysList}
            {customNote}
            {exactly}
            {details}
          </div>
        </>
      );
    return (
      <div className="space-y-2 text-sm text-ink-2">
        {level ? <p>{LEVEL_LABEL[level]}</p> : null}
        {alwaysList}
        {customNote}
        {exactly}
        {details}
      </div>
    );
  }

  // (P5-POLISH-6, C2) One select: the account's levels, Custom… and What exactly? (both open the actions table).
  const levels = LEVEL_ORDER.filter((l) => account.levels.includes(l));
  const CUSTOM = "__custom";
  const EXACTLY = "__exactly";
  const picker =
      levels.length === 1 ? (
        <p className={cn("text-sm text-ink", inRow && "whitespace-nowrap")}>{LEVEL_LABEL[levels[0]!]}</p>
      ) : (
        <span className={cn("inline-flex items-center gap-2", inRow && "max-[599px]:min-w-0 max-[599px]:flex-1")}>
        <select
          aria-label={`${entry.agent.name}'s access to ${account.name}`}
          className={cn(
            "h-9 rounded-control border border-line-strong bg-surface px-2 text-sm text-ink pointer-coarse:h-11",
            inRow ? "w-auto max-w-56 max-[599px]:w-full max-[599px]:max-w-none" : "w-full max-w-xs",
          )}
          disabled={put.isPending}
          value={entry.level === "custom" ? CUSTOM : (level ?? "")}
          onChange={(e) => {
            const v = e.target.value;
            if (v === CUSTOM || v === EXACTLY) setTable(true);
            else put.mutate({ agentId: entry.agent.id, connectionId: account.ref.id, body: { level: v as Level } }, { onSuccess: () => saved.flash() });
          }}>
          {level === null && entry.level !== "custom" ? <option value="">Choose…</option> : null}
          {levels.map((l) => (
            <option key={l} value={l}>
              {LEVEL_LABEL[l]}
            </option>
          ))}
          <option value={CUSTOM}>Custom…</option>
          <option value={EXACTLY}>What exactly?</option>
        </select>
        <InlineSaved show={saved.saved} />
        </span>
      );
  const rest = (
    <>
      {isLimited(account) ? (
        <div className="text-sm text-ink-2">
          <p>Limited by {orgName ?? "your organization"}.</p>
          {account.managed_by ? <WhoCan value={account.managed_by} /> : null}
        </div>
      ) : null}
      {alwaysList}
      {customNote}
      {put.isError ? <ErrorState error={put.error} /> : null}
      {details}
    </>
  );
  if (inRow)
    return (
      <>
        {picker}
        <div className="order-last w-full basis-full space-y-2">{rest}</div>
      </>
    );
  return (
    <div className="w-full space-y-2">
      {picker}
      {rest}
    </div>
  );
}
