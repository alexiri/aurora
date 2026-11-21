"use client";

import { useQueries, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";

import { BulkBar } from "@/components/ui/bulk-bar";
import { Button } from "@/components/ui/button";
import { ConfirmDialog } from "@/components/ui/confirm-dialog";
import { Notice } from "@/components/ui/notice";
import { Select } from "@/components/ui/select";
import { accountKeys, useGiveSecret, usePutAccess, type Account, type Level } from "@/lib/api/accounts";
import { api } from "@/lib/api/client";
import { queryKeys, useAgents, useMe, useWorkspaces } from "@/lib/api/queries";
import type { SecretScope } from "@/lib/api/secrets";
import { unwrap } from "@/lib/api/unwrap";
import { Field } from "@/components/ui/field";
import { useBulk } from "@/lib/bulk";
import { workspaceDisplayName } from "@/lib/workspace-name";

/** Why an account cannot be given to an agent in bulk, or `undefined`. A custom-only app has no level to pick here. */
export function cannotGive(a: Account): string | undefined {
  if (!a.can.give_access) return "You cannot give this account to agents";
  if (a.ref.type === "connection" && a.levels.length === 0) return "Choose what an agent may do with it on the agent's own page";
  return undefined;
}

/** Why an account cannot be turned off in bulk, or `undefined`: only an app account that is on has a switch. */
export function cannotTurnOff(a: Account): string | undefined {
  if (a.ref.type !== "connection") return "Only an app account has a switch";
  if (!a.can.manage) return "You cannot turn this account off";
  if (a.health.state === "disabled") return "Already off";
  return undefined;
}

interface PickAgent {
  id: string;
  label: string;
}

/** The agents an account can be given to: the workspace's own; for an organization's account, those of the workspaces the person owns. */
function usePickableAgents(scope: SecretScope, orgId: string): { agents: PickAgent[]; loading: boolean } {
  const me = useMe();
  const wsAgents = useAgents(scope.kind === "workspace" ? scope.id : undefined);
  const workspaces = useWorkspaces(scope.kind === "organization" ? orgId : undefined);
  const owned = (workspaces.data?.items ?? []).filter((w) => w.my_role === "owner");
  const lists = useQueries({
    queries: owned.map((w) => ({
      queryKey: queryKeys.agents(w.id),
      queryFn: () => unwrap(api.GET("/v1/workspaces/{ws}/agents", { params: { path: { ws: w.id } } })),
    })),
  });
  if (scope.kind === "workspace") return { agents: (wsAgents.data?.items ?? []).map((a) => ({ id: a.id, label: a.name })), loading: wsAgents.isPending };
  const agents = owned.flatMap((w, i) => (lists[i]?.data?.items ?? []).map((a) => ({ id: a.id, label: `${workspaceDisplayName(w, me.data?.user)} · ${a.name}` })));
  return { agents, loading: workspaces.isPending || lists.some((l) => l.isPending) };
}

/**
 * (P5-REFINE-19, `bulk`, Expert) The Accounts list's bar: **Give to <agent>…** (a picker) and **Turn off**, each asking once with
 * the count, then the per-account routes in turn ("2 of 3"). `picked` are the ticked accounts.
 */
export function AccountsBulkBar({ picked, scope, orgId, onClear }: { picked: Account[]; scope: SecretScope; orgId: string; onClear: () => void }) {
  const qc = useQueryClient();
  const bulk = useBulk();
  const [dialog, setDialog] = useState<"give" | "off" | null>(null);
  const [agentId, setAgentId] = useState("");
  const { agents, loading } = usePickableAgents(scope, orgId);
  const put = usePutAccess();
  const give = useGiveSecret();

  const n = picked.length;
  const noun = n === 1 ? "account" : "accounts";
  const count = `${n} ${noun}`;
  const byId = new Map(picked.map((a) => [a.ref.id, a]));
  const givable = picked.filter((a) => cannotGive(a) === undefined);
  const offable = picked.filter((a) => cannotTurnOff(a) === undefined);
  const agent = agents.find((a) => a.id === agentId);
  const refresh = () => {
    void qc.invalidateQueries({ queryKey: accountKeys.all });
    void qc.invalidateQueries({ queryKey: ["apps"] });
    void qc.invalidateQueries({ queryKey: ["connections"] });
  };

  const run = async (action: (a: Account) => Promise<unknown>, of: Account[]) => {
    await bulk.run(
      of.map((a) => ({ id: a.ref.id, name: a.name })),
      (t) => action(byId.get(t.id)!),
    );
    refresh();
  };

  return (
    <>
      <BulkBar count={n} noun={n === 1 ? "account selected" : "accounts selected"} bulk={bulk} onClear={onClear}>
        <Button type="button" size="sm" variant="outline" disabled={bulk.running || givable.length === 0} onClick={() => { setAgentId(agents[0]?.id ?? ""); setDialog("give"); }}>
          Give to an agent…
        </Button>
        <Button type="button" size="sm" variant="outline" disabled={bulk.running || offable.length === 0} onClick={() => setDialog("off")}>
          Turn off…
        </Button>
      </BulkBar>
      <ConfirmDialog
        open={dialog === "give"}
        onOpenChange={(o) => !o && setDialog(null)}
        title={`Give ${givable.length === 1 ? "1 account" : `${givable.length} accounts`} to ${agent ? agent.label : "an agent"}?`}
        description="The agent can use them as soon as you confirm. You can take each one back from the agent's Apps & access page."
        confirmLabel={agent ? `Give to ${agent.label}` : "Give"}
        busy={!agent}
        onConfirm={() => {
          if (!agent) return;
          const id = agent.id;
          setDialog(null);
          void run(
            (a) =>
              a.agents.some((x) => x.agent.id === id)
                ? Promise.resolve()
                : a.ref.type === "connection"
                  ? put.mutateAsync({ agentId: id, connectionId: a.ref.id, body: { level: (a.levels.includes("read") ? "read" : a.levels[0]) as Level } })
                  : give.mutateAsync({ agentId: id, secretId: a.ref.id }),
            givable,
          );
        }}
      >
        <div className="space-y-3 text-sm">
          <Field id="bulk-give-agent" label="Agent">
            <Select id="bulk-give-agent" value={agentId} onChange={setAgentId} disabled={loading} placeholder={loading ? "Loading…" : agents.length === 0 ? "No agents yet" : undefined} options={agents.map((a) => ({ value: a.id, label: a.label }))} />
          </Field>
          <p className="text-ink-2">Apps are given at the lowest level that exists (read only); websites and AI keys as they are.</p>
          {givable.length < n ? (
            <Notice tone="pending" role="status">
              {n - givable.length} of the {count} cannot be given here: {picked.filter((a) => cannotGive(a)).map((a) => `${a.name} (${cannotGive(a)})`).join("; ")}.
            </Notice>
          ) : null}
        </div>
      </ConfirmDialog>
      <ConfirmDialog
        open={dialog === "off"}
        onOpenChange={(o) => !o && setDialog(null)}
        title={`Turn off ${offable.length === 1 ? "1 account" : `${offable.length} accounts`}?`}
        description="Agents that use them stop using them now. Turn each one back on from its menu; nothing is deleted."
        confirmLabel={`Turn off ${offable.length === 1 ? "1 account" : `${offable.length} accounts`}`}
        danger
        onConfirm={() => {
          setDialog(null);
          void run((a) => unwrap(api.PATCH("/v1/connections/{id}", { params: { path: { id: a.ref.id } }, body: { status: "disabled" } })), offable);
        }}
      >
        {offable.length < n ? (
          <Notice tone="pending" role="status">
            {n - offable.length} of the {count} have no switch to turn off: {picked.filter((a) => cannotTurnOff(a)).map((a) => `${a.name} (${cannotTurnOff(a)})`).join("; ")}.
          </Notice>
        ) : null}
      </ConfirmDialog>
    </>
  );
}
