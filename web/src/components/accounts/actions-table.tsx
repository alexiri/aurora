"use client";

import Link from "next/link";
import { Altitude } from "@/components/ui/altitude";
import { useParams } from "next/navigation";
import { useMemo, useState } from "react";

import { ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Notice } from "@/components/ui/notice";
import { Select } from "@/components/ui/select";
import { WhoCan } from "@/components/ui/who-can";
import { type Account, type AgentAccess, type AppAccessTool, useAppAccess, usePutAccess } from "@/lib/api/accounts";
import { useGrants } from "@/lib/api/policy";
import { useApp } from "@/lib/api/apps";
import { TechnicalDetails } from "@/components/ui/technical-details";
import { counts, customBody, groupOf, initialDecisions, type Decision, type Tool } from "@/lib/grant-editor";
import { useOrgMemberOnly } from "@/lib/nav/org-member";
import { orgRoutes, wsRoutes } from "@/lib/routes";

const DECISION_LABEL: Record<Decision, string> = { allow: "Allowed", ask: "Asks you", block: "Blocked" };
const DECISION_TONE: Record<Decision, "neutral" | "danger"> = { allow: "neutral", ask: "neutral", block: "danger" };

/** The table's starting choices: the saved ones; an `act` choice allows everything; a fresh `custom_only` account allows reads (C-410). */
export function startingDecisions(
  tools: Tool[],
  entry: Pick<AgentAccess, "level"> | undefined,
  saved: { mode: "full" | "custom"; allow_tools: string[]; ask_tools: string[] } | undefined,
  customOnly: boolean,
): Record<string, Decision> {
  // (P5-TOOLASK, C-1043) An Act choice allows everything except what its app asks about first; choosing Allowed on such an action is the override.
  if (saved?.mode === "full" || (!saved && entry?.level === "act"))
    return Object.fromEntries(tools.map((t) => [t.id, (t.approval === "ask" ? "ask" : "allow") as Decision]));
  const d = initialDecisions(tools, saved);
  if (!saved && customOnly) for (const t of tools) if (groupOf(t) === "read") d[t.id] = "allow";
  return d;
}

/**
 * The actions table (C-938, C-934): every action of an app with what actually happens (Allowed, Asks you, Blocked)
 * and, only when something other than the agent's access decided, why. It opens inside the account's row, from
 * **What exactly?** or **Custom…** (one table, not two). People who may change the access set each action in place
 * (one save, C-980: a change set to Allowed under Ask before changes is the same as Always allow, and the server
 * reads it back as Ask with that change always allowed, not as Custom); everyone else reads it. Methods and host
 * patterns are under Technical details. At least one action must be allowed or asked.
 */
export function ActionsTable({
  account,
  entry,
  agentId,
  agentName,
  workspaceId,
  canChange,
  onClose,
}: {
  account: Account;
  entry?: AgentAccess;
  agentId: string;
  agentName: string;
  workspaceId: string;
  canChange: boolean;
  onClose: () => void;
}) {
  const access = useAppAccess(agentId, account.app?.app_id);
  if (access.isPending) return <Loading />;
  if (access.isError) return <ErrorState error={access.error} onRetry={() => access.refetch()} />;
  const mine = access.data.accounts.find((a) => a.ref.id === account.ref.id);
  const rows = mine?.tools ?? [];
  return (
    <section aria-label={`What ${agentName} can do in ${account.app?.name ?? account.name}`} className="space-y-3 rounded-card border border-line bg-canvas p-3">
      <p className="text-sm text-ink-2">
        What {agentName} can do in {account.app?.name ?? account.name}, action by action.
        {canChange ? " Most people never need this: the three choices cover almost everything." : null}
      </p>
      {rows.length === 0 ? <p className="text-sm text-muted">This app has no actions to list.</p> : null}
      {canChange ? (
        <Editable account={account} entry={entry} agentId={agentId} agentName={agentName} workspaceId={workspaceId} rows={rows} onClose={onClose} />
      ) : (
        <Groups rows={rows} account={account} agentName={agentName} />
      )}
      {canChange ? null : (
        <div className="flex justify-end">
          <Button variant="outline" size="sm" type="button" onClick={onClose}>
            Close
          </Button>
        </div>
      )}
    </section>
  );
}

const GROUPS = [
  { id: "read", label: "Looking things up" },
  { id: "write", label: "Changing things" },
] as const;

/** (P5-TOOLASK, C-1042) The app's own default for an action that acts on other people: it asks first until a workspace says Allowed. */
function AsksFirst({ tool, canChange }: { tool: { approval?: string | null }; canChange: boolean }) {
  if (tool.approval !== "ask") return null;
  return (
    <p className="text-xs text-muted">
      Asks first (app default).{canChange ? " Choose Allowed to let the agent do it without asking." : null}
    </p>
  );
}

/** The read-only table: the chip says what happens; a reason sentence and the agent's own choice follow it when something else decided. */
function Groups({ rows, account, agentName }: { rows: AppAccessTool[]; account: Account; agentName: string }) {
  return (
    <div className="space-y-4">
      {GROUPS.map((g) => {
        const list = rows.filter((t) => t.access === g.id);
        if (list.length === 0) return null;
        return (
          <section key={g.id} aria-label={g.label} className="space-y-2">
            <h4 className="text-sm font-semibold text-ink">{g.label}</h4>
            <ul className="divide-y divide-line rounded-card border border-line bg-surface">
              {list.map((t) => (
                <li key={t.tool_id} className="flex flex-wrap items-start justify-between gap-x-4 gap-y-2 p-3">
                  <div className="min-w-0 flex-1 basis-56">
                    <p className="text-sm font-medium text-ink">{t.name}</p>
                    {t.description ? <p className="text-sm text-ink-2">{t.description}</p> : null}
                  </div>
                  <div className="space-y-1 sm:text-right">
                    <Badge tone={DECISION_TONE[t.decision as Decision]}>{DECISION_LABEL[t.decision as Decision]}</Badge>
                    <Reason tool={t} account={account} agentName={agentName} showOwn />
                  </div>
                </li>
              ))}
            </ul>
          </section>
        );
      })}
    </div>
  );
}

/** "Limited by Acme", or the rule's name with a link to Internet access; the agent's own choice in small text. */
function Reason({ tool, account, agentName, showOwn }: { tool: AppAccessTool; account: Account; agentName: string; showOwn: boolean }) {
  const params = useParams<{ org?: string; ws?: string }>();
  // (D-508(e)) A member doesn't see Manage: their link is the workspace's page, which shows what applies from above.
  const member = useOrgMemberOnly(params.org);
  const r = tool.reason;
  if (!r) return null;
  const href =
    r.link !== "internet_access"
      ? null
      : (r.layer === "workspace_rule" || member) && params.org && params.ws
        ? wsRoutes(params.org, params.ws).internetAccess()
        : params.org
          ? member
            ? null
            : orgRoutes(params.org).internetAccess()
          : "/admin/internet-access";
  return (
    <div className="space-y-0.5 text-sm text-ink-2">
      <p>
        {r.layer === "org_rule" || r.layer === "workspace_rule" ? "Rule: " : null}
        {r.label}
        {href ? (
          <>
            {" · "}
            <Link href={href} className="underline">
              Internet access
            </Link>
          </>
        ) : null}
      </p>
      {r.layer === "limit" && account.managed_by ? <WhoCan value={account.managed_by} /> : null}
      {showOwn ? (
        <p className="text-xs text-muted">
          {agentName}&apos;s access: {DECISION_LABEL[tool.own as Decision]}
        </p>
      ) : null}
    </div>
  );
}

function Editable({
  account,
  entry,
  agentId,
  agentName,
  workspaceId,
  rows,
  onClose,
}: {
  account: Account;
  entry?: AgentAccess;
  agentId: string;
  agentName: string;
  workspaceId: string;
  rows: AppAccessTool[];
  onClose: () => void;
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
      agentName={agentName}
      tools={app.data.tools}
      rows={rows}
      saved={saved}
      customOnly={account.app?.custom_only ?? false}
      onClose={onClose}
    />
  );
}

function Editor({
  account,
  entry,
  agentId,
  agentName,
  tools,
  rows,
  saved,
  customOnly,
  onClose,
}: {
  account: Account;
  entry?: AgentAccess;
  agentId: string;
  agentName: string;
  tools: Tool[];
  rows: AppAccessTool[];
  saved: { mode: "full" | "custom"; allow_tools: string[]; ask_tools: string[]; ask_timeout_secs?: number | null } | undefined;
  customOnly: boolean;
  onClose: () => void;
}) {
  const put = usePutAccess();
  const [decisions, setDecisions] = useState(() => startingDecisions(tools, entry, saved, customOnly));
  const c = counts(decisions);
  const byId = useMemo(() => new Map(rows.map((r) => [r.tool_id, r])), [rows]);
  const groups = useMemo(
    () => GROUPS.map((g) => ({ ...g, tools: tools.filter((t) => groupOf(t) === g.id) })),
    [tools],
  );
  const empty = c.allow + c.ask === 0;
  return (
    <div className="space-y-4">
      {groups.map((g) =>
        g.tools.length === 0 ? null : (
          <section key={g.id} aria-label={g.label} className="space-y-2">
            <h4 className="text-sm font-semibold text-ink">{g.label}</h4>
            <ul className="divide-y divide-line rounded-card border border-line bg-surface">
              {g.tools.map((t) => {
                const row = byId.get(t.id);
                return (
                  <li key={t.id} className="flex flex-wrap items-start justify-between gap-x-4 gap-y-2 p-3">
                    <div className="min-w-0 flex-1 basis-56">
                      <p className="text-sm font-medium text-ink">{t.name}</p>
                      {t.description ? <p className="text-sm text-ink-2">{t.description}</p> : null}
                      <AsksFirst tool={t} canChange />
                      <Altitude id="tech.details" level="setup">
                        <TechnicalDetails className="mt-1 text-xs" bodyClassName="mt-1 break-all font-mono">
                          {t.methods.join(", ") || "any method"} · {t.host}
                          {t.path}
                        </TechnicalDetails>
                      </Altitude>
                    </div>
                    <div className="space-y-1 sm:text-right">
                      <Select
                        aria-label={`${t.name}: what happens`}
                        className="min-h-9 w-auto px-2 py-1"
                        value={decisions[t.id] ?? "block"}
                        onChange={(v) => setDecisions({ ...decisions, [t.id]: v as Decision })}
                        options={(["allow", "ask", "block"] as const).map((d) => ({ value: d, label: DECISION_LABEL[d] }))}
                      />
                      {/* Something else decided: what actually happens, and why. The select above is the agent's own choice. */}
                      {row?.reason ? (
                        <div className="space-y-1">
                          <p>
                            <Badge tone={DECISION_TONE[row.decision as Decision]}>{DECISION_LABEL[row.decision as Decision]}</Badge>
                          </p>
                          <Reason tool={row} account={account} agentName={agentName} showOwn={false} />
                        </div>
                      ) : null}
                    </div>
                  </li>
                );
              })}
            </ul>
          </section>
        ),
      )}
      {empty ? <Notice tone="pending">Allow or ask for at least one action, or remove the access instead.</Notice> : null}
      {put.isError ? <ErrorState error={put.error} /> : null}
      <div className="flex flex-wrap justify-end gap-2">
        <Button variant="outline" size="sm" type="button" onClick={onClose}>
          Cancel
        </Button>
        <Button
          type="button"
          size="sm"
          disabled={empty || put.isPending}
          onClick={() =>
            put.mutate(
              {
                agentId,
                connectionId: account.ref.id,
                body: customBody(decisions, saved?.ask_timeout_secs != null ? String(saved.ask_timeout_secs) : ""),
              },
              { onSuccess: onClose },
            )
          }
        >
          {put.isPending ? "Saving…" : "Save"}
        </Button>
      </div>
    </div>
  );
}
