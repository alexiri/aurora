"use client";

import { useEffect, useMemo, useState } from "react";

import { AccessDialog, type GrantSubject } from "@/components/credentials/give-access";
import { ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import { Notice } from "@/components/ui/notice";
import { splitWorkspaces } from "@/lib/accounts";
import { type Account, type Level, usePutAccess } from "@/lib/api/accounts";
import { errorMessage } from "@/lib/api/errors";
import { useAgents, useMe, useWorkspaces } from "@/lib/api/queries";

import { AccessChoice } from "./access-choice";

type AgentRow = { id: string; name: string; handle: string; workspace: string };

/** One owned workspace: its agents as rows "<workspace> · <agent>", each ticked agent with its own access choice. */
function WorkspaceRows({
  wsId,
  wsName,
  account,
  orgName,
  picks,
  has,
  toggle,
  choose,
  disabled,
  register,
}: {
  wsId: string;
  wsName: string;
  account: Account;
  orgName?: string;
  picks: Record<string, Level>;
  has: Set<string>;
  toggle: (agent: AgentRow) => void;
  choose: (agentId: string, level: Level) => void;
  disabled: boolean;
  register: (agents: AgentRow[]) => void;
}) {
  const agents = useAgents(wsId);
  const rows = useMemo<AgentRow[]>(() => (agents.data?.items ?? []).map((a) => ({ id: a.id, name: a.name, handle: a.handle, workspace: wsName })), [agents.data, wsName]);
  useEffect(() => register(rows), [rows, register]);
  if (agents.isPending) return <Loading />;
  if (agents.isError) return <ErrorState error={agents.error} onRetry={() => void agents.refetch()} />;
  if (rows.length === 0) return null;
  return (
    <ul className="space-y-2">
      {rows.map((a) => {
        const already = has.has(a.id);
        const ticked = a.id in picks;
        return (
          <li key={a.id} className="space-y-2">
            <label className="flex min-h-9 items-center gap-2.5 text-sm">
              <input
                type="checkbox"
                className="size-4 shrink-0 accent-accent"
                checked={already || ticked}
                disabled={already || disabled}
                onChange={() => toggle(a)}
                aria-label={`Give ${a.name} access`}
              />
              <span className="min-w-0 break-words font-medium text-ink">
                {a.workspace} · {a.name}
              </span>
              {already ? <Badge tone="accent">Has access</Badge> : null}
            </label>
            {ticked ? (
              <div className="ml-6">
                <AccessChoice account={account} orgName={orgName} agentName={a.name} legend={`What may ${a.name} do?`} value={picks[a.id]!} disabled={disabled} onChange={(l) => choose(a.id, l)} />
              </div>
            ) : null}
          </li>
        );
      })}
    </ul>
  );
}

/**
 * **Give access** for an organization's account (C-936, `14 §11`): the agents of the workspaces the person owns, own
 * and shared workspaces first, rows "<workspace> · <agent>", and for each ticked agent its own access choice within the
 * account's limit. Each choice is saved like an agent's own access change (C-980). An account with no choice (a
 * custom-only app, a website, an AI key) keeps the plain picker.
 */
function OrgPicker({ account, orgId, orgName, onClose }: { account: Account; orgId: string; orgName?: string; onClose: () => void }) {
  const workspaces = useWorkspaces(orgId);
  const me = useMe();
  const put = usePutAccess();
  const [picks, setPicks] = useState<Record<string, Level>>({});
  const [names, setNames] = useState<Record<string, string>>({});
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<{ name: string; error: unknown } | null>(null);
  const has = useMemo(() => new Set(account.agents.map((a) => a.agent.id)), [account.agents]);
  const levels = (["read", "ask", "act"] as const).filter((l) => account.levels.includes(l));
  const register = useMemo(
    () => (agents: AgentRow[]) =>
      setNames((cur) => (agents.every((a) => cur[a.id] === a.name) ? cur : { ...cur, ...Object.fromEntries(agents.map((a) => [a.id, a.name])) })),
    [],
  );

  const owned = (workspaces.data?.items ?? []).filter((w) => w.my_role === "owner");
  const { mine, others } = splitWorkspaces(owned, me.data?.user.id);
  const ordered = [...mine, ...others];

  const toggle = (a: AgentRow) =>
    setPicks((cur) => {
      const next = { ...cur };
      if (a.id in next) delete next[a.id];
      else next[a.id] = levels[0] ?? "read";
      return next;
    });
  const ids = Object.keys(picks);

  async function save() {
    setSaving(true);
    setError(null);
    for (const id of ids) {
      try {
        await put.mutateAsync({ agentId: id, connectionId: account.ref.id, body: { level: picks[id]! } });
      } catch (e) {
        setError({ name: names[id] ?? "an agent", error: e });
        setSaving(false);
        return;
      }
    }
    setSaving(false);
    onClose();
  }

  if (workspaces.isPending) return <Loading />;
  if (workspaces.isError) return <ErrorState error={workspaces.error} onRetry={() => void workspaces.refetch()} />;
  if (ordered.length === 0) return <p className="text-sm text-muted">You do not own any workspace in this organization, so there are no agents to choose.</p>;
  return (
    <div className="space-y-4" aria-label="Give agents access">
      {ordered.map((w) => (
        <fieldset key={w.id} className="min-w-0 space-y-1">
          <legend className="mb-1 text-sm font-semibold text-ink">{w.name}</legend>
          <WorkspaceRows wsId={w.id} wsName={w.name} account={account} orgName={orgName} picks={picks} has={has} toggle={toggle} choose={(id, l) => setPicks((c) => ({ ...c, [id]: l }))} disabled={saving} register={register} />
        </fieldset>
      ))}
      {error ? (
        <Notice tone="danger" role="alert">
          Could not give access to {error.name}: {errorMessage(error.error)}
        </Notice>
      ) : null}
      <div className="flex flex-wrap gap-2">
        <Button size="sm" disabled={saving || ids.length === 0} onClick={() => void save()}>
          {saving ? "Giving access…" : ids.length === 1 ? `Give ${names[ids[0]!] ?? "the agent"} access` : "Give access"}
        </Button>
        <Button size="sm" variant="outline" onClick={onClose}>
          Close
        </Button>
      </div>
    </div>
  );
}

/**
 * The dialog behind **Give access** on an organization's account. Accounts that have the three choices get the picker
 * with a choice per agent; the others (custom-only apps, websites, AI keys) get the plain access dialog.
 */
export function OrgGiveAccessDialog({
  account,
  orgId,
  orgName,
  subject,
  useOrgPicker = true,
  onOpenChange,
}: {
  account: Account;
  orgId: string;
  orgName?: string;
  subject: GrantSubject;
  /** `false` where the list is a workspace's: the plain access dialog of that workspace. */
  useOrgPicker?: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  if (!useOrgPicker || account.levels.length === 0 || account.ref.type !== "connection") {
    return <AccessDialog open onOpenChange={onOpenChange} title={account.name} subject={subject} />;
  }
  return (
    <Dialog open onOpenChange={onOpenChange}>
      <DialogContent title={`Give access to ${account.name}`} description="Choose the agents and what each may do." size="md">
        <OrgPicker account={account} orgId={orgId} orgName={orgName} onClose={() => onOpenChange(false)} />
      </DialogContent>
    </Dialog>
  );
}
