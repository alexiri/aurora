"use client";

import { useMemo, useState } from "react";

import { ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import { Dialog, DialogClose, DialogContent } from "@/components/ui/dialog";
import { Notice } from "@/components/ui/notice";
import { type Account, type AgentAccess, usePutAccess } from "@/lib/api/accounts";
import { useGrants } from "@/lib/api/policy";
import { useApp } from "@/lib/api/apps";
import { counts, customBody, groupOf, initialDecisions, type Decision, type Tool } from "@/lib/grant-editor";

const DECISION_LABEL: Record<Decision, string> = { allow: "Allowed", ask: "Asks you", block: "Blocked" };

/** The table's starting choices: the saved ones; an `act` choice allows everything; a fresh `custom_only` account allows reads (C-410). */
export function startingDecisions(
  tools: Tool[],
  entry: Pick<AgentAccess, "level"> | undefined,
  saved: { mode: "full" | "custom"; allow_tools: string[]; ask_tools: string[] } | undefined,
  customOnly: boolean,
): Record<string, Decision> {
  if (saved?.mode === "full" || (!saved && entry?.level === "act")) return Object.fromEntries(tools.map((t) => [t.id, "allow" as Decision]));
  const d = initialDecisions(tools, saved);
  if (!saved && customOnly) for (const t of tools) if (groupOf(t) === "read") d[t.id] = "allow";
  return d;
}

/**
 * The actions table (C-938): every action of an app with the agent's saved choice, editable by people who may change
 * the access. Methods and host patterns are under Technical details. At least one action must be allowed or asked.
 * One save (C-980): setting a change to Allowed under Ask before changes is the same as Always allow, and the server
 * reads it back as Ask with that change always allowed, not as Custom.
 */
export function ActionsTable({
  account,
  entry,
  agentId,
  agentName,
  workspaceId,
  open,
  onOpenChange,
}: {
  account: Account;
  entry?: AgentAccess;
  agentId: string;
  agentName: string;
  workspaceId: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        size="xl"
        title={`What ${agentName} can do in ${account.app?.name ?? account.name}`}
        description="Choose, action by action. Most people never need this: the three choices cover almost everything."
      >
        {open ? (
          <Body account={account} entry={entry} agentId={agentId} workspaceId={workspaceId} onDone={() => onOpenChange(false)} />
        ) : null}
      </DialogContent>
    </Dialog>
  );
}

function Body({
  account,
  entry,
  agentId,
  workspaceId,
  onDone,
}: {
  account: Account;
  entry?: AgentAccess;
  agentId: string;
  workspaceId: string;
  onDone: () => void;
}) {
  const app = useApp(account.app?.app_id, workspaceId);
  const grants = useGrants(agentId);
  if (app.isPending || grants.isPending) return <Loading />;
  if (app.isError) return <ErrorState error={app.error} onRetry={() => app.refetch()} />;
  if (grants.isError) return <ErrorState error={grants.error} onRetry={() => grants.refetch()} />;
  const saved = grants.data.connections.find((c) => c.connection_id === account.ref.id);
  return (
    <Editor
      account={account}
      entry={entry}
      agentId={agentId}
      tools={app.data.tools}
      saved={saved}
      customOnly={account.app?.custom_only ?? false}
      onDone={onDone}
    />
  );
}

function Editor({
  account,
  entry,
  agentId,
  tools,
  saved,
  customOnly,
  onDone,
}: {
  account: Account;
  entry?: AgentAccess;
  agentId: string;
  tools: Tool[];
  saved: { mode: "full" | "custom"; allow_tools: string[]; ask_tools: string[]; ask_timeout_secs?: number | null } | undefined;
  customOnly: boolean;
  onDone: () => void;
}) {
  const put = usePutAccess();
  const [decisions, setDecisions] = useState(() => startingDecisions(tools, entry, saved, customOnly));
  const c = counts(decisions);
  const groups = useMemo(
    () => [
      { id: "read", label: "Looking things up", tools: tools.filter((t) => groupOf(t) === "read") },
      { id: "write", label: "Changing things", tools: tools.filter((t) => groupOf(t) === "write") },
    ],
    [tools],
  );
  const empty = c.allow + c.ask === 0;
  return (
    <div className="space-y-4">
      {groups.map((g) =>
        g.tools.length === 0 ? null : (
          <section key={g.id} aria-label={g.label} className="space-y-2">
            <h3 className="text-sm font-semibold text-ink">{g.label}</h3>
            <ul className="divide-y divide-line rounded-card border border-line">
              {g.tools.map((t) => (
                <li key={t.id} className="flex flex-wrap items-start justify-between gap-x-4 gap-y-2 p-3">
                  <div className="min-w-0 flex-1 basis-56">
                    <p className="text-sm font-medium text-ink">{t.name}</p>
                    {t.description ? <p className="text-sm text-ink-2">{t.description}</p> : null}
                    <details className="mt-1 text-xs text-muted">
                      <summary className="cursor-pointer">Technical details</summary>
                      <p className="mt-1 break-all">
                        {t.methods.join(", ") || "any method"} · {t.host}
                        {t.path}
                      </p>
                    </details>
                  </div>
                  <select
                    aria-label={`${t.name}: what happens`}
                    className="h-9 rounded-control border border-line bg-surface px-2 text-sm text-ink"
                    value={decisions[t.id] ?? "block"}
                    onChange={(e) => setDecisions({ ...decisions, [t.id]: e.target.value as Decision })}
                  >
                    {(["allow", "ask", "block"] as const).map((d) => (
                      <option key={d} value={d}>
                        {DECISION_LABEL[d]}
                      </option>
                    ))}
                  </select>
                </li>
              ))}
            </ul>
          </section>
        ),
      )}
      {empty ? <Notice tone="pending">Allow or ask for at least one action, or remove the access instead.</Notice> : null}
      {put.isError ? <ErrorState error={put.error} /> : null}
      <div className="flex flex-wrap justify-end gap-2">
        <DialogClose asChild>
          <Button variant="outline" type="button">
            Cancel
          </Button>
        </DialogClose>
        <Button
          type="button"
          disabled={empty || put.isPending}
          onClick={() =>
            put.mutate(
              {
                agentId,
                connectionId: account.ref.id,
                body: customBody(decisions, saved?.ask_timeout_secs != null ? String(saved.ask_timeout_secs) : ""),
              },
              { onSuccess: onDone },
            )
          }
        >
          {put.isPending ? "Saving…" : "Save"}
        </Button>
      </div>
    </div>
  );
}
