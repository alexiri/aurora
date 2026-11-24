"use client";

import { AccessChoice } from "@/components/accounts/access-choice";
import { Button } from "@/components/ui/button";
import { ChoiceCards } from "@/components/ui/choice-cards";
import { Notice } from "@/components/ui/notice";
import type { Level } from "@/lib/api/accounts";
import type { PlanEntry } from "@/lib/api/agent-templates";

import { unavailableWords } from "./words";

/**
 * One entry's account choice (C-1116, the Accounts step): which app (`any_of`), which account of it (an existing one
 * the person may give, or **Connect a new account** through the account-only connect journey), and the access
 * preselected to the template's level (capped as C-980 by the plan's accounts). Optional entries can be skipped.
 */
export interface AccountEntryState {
  /** The chosen app among the entry's choices; the first available one by default. */
  appId?: string;
  /** The chosen account: a connection id, `new` (the journey), or `skip` (optional entries only). */
  accountId?: string;
  /** The access when the person changed the template's level; absent = the plan's level. */
  level?: Level;
}

const ToolList = ({ label, tools }: { label: string; tools: string[] }) =>
  tools.length === 0 ? null : (
    <p className="text-sm text-ink-2">
      {label} <span className="font-mono text-xs [overflow-wrap:anywhere]">{tools.join(", ")}</span>
    </p>
  );

export function AccountsStep({
  entries,
  state,
  onChange,
  onConnect,
  orgName,
}: {
  entries: PlanEntry[];
  state: Record<number, AccountEntryState>;
  onChange: (index: number, next: AccountEntryState) => void;
  /** Opens the account-only connect journey for the entry's app; the page owns the dialog. */
  onConnect: (index: number, appId: string) => void;
  /** The organization that may limit an account, for "Limited by <org>". */
  orgName?: string;
}) {
  const set = (index: number, patch: Partial<AccountEntryState>) => onChange(index, { ...state[index], ...patch });
  return (
    <div className="space-y-6">
      {entries.length === 0 ? <p className="text-sm text-ink-2">The template needs no accounts. You can give the agent apps later.</p> : null}
      {entries.map((entry) => {
        const pick = state[entry.index] ?? {};
        const chosen = entry.choices.find((c) => c.app_id === pick.appId) ?? entry.choices.find((c) => c.available) ?? entry.choices[0];
        const existing = chosen?.accounts ?? [];
        const explicit = pick.accountId;
        const isNew = explicit === "new";
        const skipped = explicit === "skip";
        const chosenAccount = !isNew && !skipped && explicit ? explicit : existing[0]?.ref.id;
        const target = existing.find((a) => a.ref.id === chosenAccount);
        const customOnly = chosen?.level_note === "custom_only";
        const planLevel = (chosen?.level ?? "read") as Level;
        const levels = target?.levels ?? [];
        const value: Level | null = pick.level ?? (levels.includes(planLevel) ? planLevel : (levels[0] ?? null));
        return (
          <section key={entry.index} className="space-y-3" aria-label={entry.why}>
            <p className="text-sm font-medium text-ink">
              {entry.why}
              {entry.required ? "" : " (optional)"}
            </p>
            {!chosen ? null : !chosen.available ? (
              <Notice tone="pending">
                <p>{unavailableWords(chosen.app_name, chosen.unavailable_reason)}</p>
              </Notice>
            ) : (
              <>
                {entry.choices.length > 1 ? (
                  <ChoiceCards
                    legend="Which app?"
                    value={chosen.app_id}
                    onChange={(appId) => set(entry.index, { appId, accountId: undefined, level: undefined })}
                    options={entry.choices.map((c) => ({ value: c.app_id, label: c.app_name, disabled: !c.available }))}
                  />
                ) : null}
                <ChoiceCards<string>
                  legend={`Which ${chosen.app_name} account?`}
                  value={isNew ? "new" : skipped ? "skip" : (chosenAccount ?? "new")}
                  onChange={(v) => {
                    if (v === "new") {
                      set(entry.index, { accountId: "new", level: undefined });
                      onConnect(entry.index, chosen.app_id);
                    } else set(entry.index, { accountId: v, level: undefined });
                  }}
                  options={[
                    ...existing.map((a, i) => ({
                      value: a.ref.id,
                      label: `Use ${a.name}`,
                      badge: i === 0 && explicit == null ? "Recommended" : undefined,
                    })),
                    { value: "new", label: `Connect a new ${chosen.app_name} account` },
                    ...(entry.required ? [] : [{ value: "skip", label: `Skip ${chosen.app_name}` }]),
                  ]}
                />
                {isNew ? (
                  <p className="flex flex-wrap items-center gap-2 text-sm text-ink-2">
                    Not connected yet.
                    <Button variant="outline" size="sm" onClick={() => onConnect(entry.index, chosen.app_id)}>
                      Connect {chosen.app_name}
                    </Button>
                  </p>
                ) : null}
                {customOnly && chosen.custom ? (
                  <div className="space-y-1 rounded-card border border-line bg-surface p-3 text-sm">
                    <p className="font-medium text-ink">The organization limits this app: it asks you before each change.</p>
                    <ToolList label="Allowed without asking:" tools={chosen.custom.allow_tools} />
                    <ToolList label="Needs your approval:" tools={chosen.custom.ask_tools} />
                    <ToolList label="Not available here:" tools={chosen.custom.dropped_tools} />
                  </div>
                ) : target && levels.length > 0 ? (
                  <AccessChoice
                    account={{ levels, limit: target.limit, app: target.app, managed_by: target.managed_by }}
                    agentName="the agent"
                    value={value}
                    onChange={(level) => set(entry.index, { level })}
                    legend={`What may the agent do in ${chosen.app_name}?`}
                    orgName={orgName}
                  />
                ) : null}
              </>
            )}
          </section>
        );
      })}
    </div>
  );
}
