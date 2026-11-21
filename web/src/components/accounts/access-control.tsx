"use client";

import { useState } from "react";

import { ErrorState } from "@/components/states";
import { Button } from "@/components/ui/button";
import { InlineSaved, useSaveBar } from "@/components/ui/save-bar";
import { WhoCan } from "@/components/ui/who-can";
import { type Account, type AgentAccess, type Level, usePutAccess } from "@/lib/api/accounts";
import { useGrants } from "@/lib/api/policy";
import { cn } from "@/lib/utils";
import { isLimited, LEVEL_LABEL, levelLabel, LEVEL_ORDER } from "@/lib/accounts";

import { ActionsTable } from "./actions-table";

/**
 * (P5-REFINE-13, access-12) The level of one agent's access, the same control on every app row: a pill that opens a select.
 * With one level the select holds only it, checked. Someone who cannot change it sees the pill without the chevron.
 */
function LevelPill({
  label,
  options,
  value,
  canChange,
  inRow,
}: {
  label: string;
  options: { value: string; text: string }[];
  value: string;
  canChange: boolean;
  inRow?: boolean;
}) {
  const pill = "h-9 rounded-control border border-line-strong bg-surface px-3 text-sm text-ink pointer-coarse:h-11";
  if (!canChange)
    return (
      <span aria-label={label} role="group" className={cn(pill, "inline-flex items-center whitespace-nowrap")} data-testid="level-pill">
        {options.find((o) => o.value === value)?.text}
      </span>
    );
  return (
    <select
      aria-label={label}
      data-testid="level-pill"
      // On a phone the select takes the line's room and the … menu sits beside it (access-2), never a line of its own.
      className={cn(pill, "w-auto", inRow && "max-sm:min-w-0 max-sm:flex-1")}
      value={value}
      onChange={() => undefined}
    >
      {options.map((o) => (
        <option key={o.value} value={o.value}>
          {o.text}
        </option>
      ))}
    </select>
  );
}

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
  const sendOnly = account.app?.send_only === true;
  // (P5-POLISH-11, P5-REFINE-13 access-12) An app with no write tools has no choice, but the row still carries the same
  // control as every other app row: the pill, with its one entry checked (no chevron when the person cannot change it).
  if (account.levels.length === 0)
    return (
      <LevelPill
        label={`${entry.agent.name}'s access to ${account.name}`}
        options={[{ value: "read", text: "Read only" }]}
        value="read"
        canChange={canChange}
        inRow={inRow}
      />
    );
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
          {level ? (
            <LevelPill label={`${entry.agent.name}'s access to ${account.name}`} options={[{ value: level, text: levelLabel(level, sendOnly) }]} value={level} canChange={false} inRow />
          ) : null}
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
        {level ? <p>{levelLabel(level, sendOnly)}</p> : null}
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
        // One level: the same pill, its one entry checked and no other entry (access-12).
        <LevelPill label={`${entry.agent.name}'s access to ${account.name}`} options={[{ value: levels[0]!, text: levelLabel(levels[0]!, sendOnly) }]} value={levels[0]!} canChange inRow={inRow} />
      ) : (
        <span className={cn("inline-flex items-center gap-2", inRow && "max-sm:min-w-0 max-sm:flex-1")}>
        <select
          aria-label={`${entry.agent.name}'s access to ${account.name}`}
          className={cn(
            "h-9 rounded-control border border-line-strong bg-surface px-2 text-sm text-ink pointer-coarse:h-11",
            inRow ? "w-auto max-w-56 max-sm:w-full max-sm:min-w-0 max-sm:max-w-none" : "w-full max-w-xs",
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
              {levelLabel(l, sendOnly)}
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
