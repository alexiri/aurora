"use client";

import { useQueries } from "@tanstack/react-query";

import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import { inputClass } from "@/components/ui/field";
import { api } from "@/lib/api/client";
import { unwrap } from "@/lib/api/unwrap";
import { useDmDefaults, useMyChannelLinks, useSetDmDefault, useUnlinkMine } from "@/lib/api/channels";
import { useSession } from "@/lib/api/queries";

import { MutationError } from "./shared";

/** Agents in every workspace the caller can use; the server decides what is actually reachable. */
function useCandidateAgents() {
  const session = useSession();
  const workspaces = (session.data?.memberships ?? []).flatMap((m) => m.workspaces.map((w) => ({ org: m.org.name, ...w })));
  const results = useQueries({
    queries: workspaces.map((w) => ({
      queryKey: ["workspaces", w.id, "agents"] as const,
      queryFn: () => unwrap(api.GET("/v1/workspaces/{ws}/agents", { params: { path: { ws: w.id } } })),
      retry: false,
    })),
  });
  const agents = results.flatMap((r, i) =>
    (r.data?.items ?? []).map((a) => ({ id: a.id, label: `${a.name} (@${a.handle}) · ${workspaces[i].name}` })),
  );
  return { agents, pending: session.isPending || results.some((r) => r.isPending) };
}

export function AccountChannelsView() {
  const links = useMyChannelLinks();
  const defaults = useDmDefaults();
  const unlink = useUnlinkMine();
  const setDefault = useSetDmDefault();
  const { agents, pending: agentsPending } = useCandidateAgents();

  if (links.isPending || defaults.isPending) return <Loading />;
  if (links.isError) return <ErrorState error={links.error} onRetry={() => links.refetch()} />;
  if (defaults.isError) return <ErrorState error={defaults.error} onRetry={() => defaults.refetch()} />;

  const dmFor = (integrationId: string, ref: string) =>
    defaults.data.items.find((d) => d.integration_id === integrationId && d.platform_user_ref === ref);

  return (
    <div className="space-y-4">
      <p className="text-sm text-neutral-600">
        Chat accounts linked to you, and which agent answers when you message the shared bot directly.
      </p>
      {links.data.items.length === 0 ? (
        <EmptyState title="No linked chat accounts">
          Message the bot in your chat platform to get a sign-in link.
        </EmptyState>
      ) : (
        <ul aria-label="Linked chat accounts" className="space-y-3">
          {links.data.items.map((l) => {
            const d = dmFor(l.integration_id, l.platform_user_ref);
            const selectId = `dm-${l.id}`;
            return (
              <li key={l.id} className="space-y-2 rounded-md border border-neutral-200 p-3">
                <div className="flex flex-wrap items-center gap-2 text-sm">
                  <span className="font-medium">{l.tenant_label}</span>
                  <code className="text-xs">{l.platform_user_ref}</code>
                  <span className="text-xs text-neutral-500">{l.method.replace(/_/g, " ")}</span>
                  <Button
                    variant="outline"
                    size="sm"
                    className="ml-auto"
                    aria-label={`Unlink ${l.tenant_label}`}
                    disabled={unlink.isPending}
                    onClick={() => unlink.mutate(l.id)}
                  >
                    Unlink
                  </Button>
                </div>
                <div>
                  <label htmlFor={selectId} className="block text-sm font-medium">
                    Default agent for direct messages
                  </label>
                  <select
                    id={selectId}
                    className={inputClass}
                    value={d?.agent_id ?? ""}
                    disabled={setDefault.isPending || agentsPending}
                    onChange={(e) =>
                      e.target.value &&
                      setDefault.mutate({ integration_id: l.integration_id, agent_id: e.target.value })
                    }
                  >
                    <option value="">{d?.agent_id ? "Select…" : "None chosen"}</option>
                    {agents.map((a) => (
                      <option key={a.id} value={a.id}>
                        {a.label}
                      </option>
                    ))}
                  </select>
                </div>
              </li>
            );
          })}
        </ul>
      )}
      <MutationError error={unlink.error ?? setDefault.error} />
    </div>
  );
}
