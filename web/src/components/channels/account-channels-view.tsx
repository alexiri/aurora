"use client";

import { useQueries } from "@tanstack/react-query";
import { useState } from "react";

import { SendAsMeSection } from "@/components/account/send-as-me";
import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { inputClass } from "@/components/ui/field";
import { Section } from "@/components/ui/section";
import { api } from "@/lib/api/client";
import { unwrap } from "@/lib/api/unwrap";
import { useDmDefaults, useMyChannelLinks, useMyChatPlatforms, useSetDmDefault, useUnlinkMine } from "@/lib/api/channels";
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

/**
 * (P5-REFINE-16, account-3, C-1089) One row per connected chat platform, naming the platform and the bot to message.
 * It never blocks the page: while it loads or fails, the linked accounts below still show.
 */
function PlatformsSection() {
  const q = useMyChatPlatforms();
  const [open, setOpen] = useState<string | null>(null);
  return (
    <Section id="chat-platforms" title="Chat platforms" description="Link your chat account so your agents recognise you there.">
      {q.isPending ? <Loading /> : null}
      {q.isError ? <p className="text-sm text-ink-2">The chat platforms could not be loaded. Reload the page to try again.</p> : null}
      {q.data && q.data.platforms.length === 0 ? (
        <p className="text-sm text-ink-2">No chat platform is connected yet. An admin can connect one.</p>
      ) : null}
      {q.data && q.data.platforms.length > 0 ? (
        <ul aria-label="Chat platforms" className="divide-y divide-line rounded-card border border-line bg-surface shadow-card">
          {q.data.platforms.map((p) => (
            <li key={p.kind} className="space-y-2 p-3 sm:px-4">
              <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
                <p className="min-w-0 flex-1 text-sm text-ink-2">
                  <span className="font-medium text-ink">{p.display_name}</span> ·{" "}
                  {p.bot_handle ? (
                    <>
                      Message <span className="font-mono text-ink">{p.bot_handle}</span> in {p.display_name} and send <code className="font-mono text-ink">link</code>.
                    </>
                  ) : (
                    <>
                      Message the bot in {p.display_name} and send <code className="font-mono text-ink">link</code>.
                    </>
                  )}
                </p>
                {p.linked ? (
                  <Badge tone="accent">Linked</Badge>
                ) : (
                  <Button variant="outline" size="sm" aria-expanded={open === p.kind} onClick={() => setOpen(open === p.kind ? null : p.kind)}>
                    Link account
                  </Button>
                )}
              </div>
              {open === p.kind && !p.linked ? (
                <p className="text-sm text-ink-2">
                  The bot replies with a sign-in link. Open it and confirm, and this account appears under Linked chat accounts.
                </p>
              ) : null}
            </li>
          ))}
        </ul>
      ) : null}
    </Section>
  );
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

  const linked = (
    <Section
      id="my-channels"
      title="Linked chat accounts"
      description="Chat accounts linked to you, and which agent answers when you message the shared bot directly."
    >
      <div className="space-y-3">
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
                <li key={l.id} className="space-y-3 rounded-card border border-line bg-surface p-4 shadow-card">
                  <div className="flex flex-wrap items-center gap-x-3 gap-y-2 text-sm">
                    <span className="font-medium text-ink">{l.tenant_label}</span>
                    <code className="font-mono text-xs text-ink-2">{l.platform_user_ref}</code>
                    <span className="text-xs text-muted">{l.method.replace(/_/g, " ")}</span>
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
                  <div className="max-w-md space-y-1.5">
                    <label htmlFor={selectId} className="block text-sm font-medium text-ink">
                      Agent that answers direct messages
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
    </Section>
  );

  return (
    <div className="space-y-8">
      <PlatformsSection />
      {linked}
      <SendAsMeSection links={links.data.items} agents={agents} />
    </div>
  );
}
