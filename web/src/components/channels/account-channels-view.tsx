"use client";

import { useQueries } from "@tanstack/react-query";
import { useState } from "react";

import { SendAsMeSection } from "@/components/account/send-as-me";
import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Field } from "@/components/ui/field";
import { Section } from "@/components/ui/section";
import { Select } from "@/components/ui/select";
import { api } from "@/lib/api/client";
import { unwrap } from "@/lib/api/unwrap";
import { useDmDefaultOptions, useDmDefaults, useMyChannelLinks, useMyChatPlatforms, useSetDmDefault, useUnlinkMine } from "@/lib/api/channels";
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

/**
 * (FIX-220, D-461(d), C-1147) The agent that answers the person's direct messages to one integration's shared bot:
 * only the agents of that integration's organization the person can use. Choosing one that is not on the bot yet
 * adds it there (the server attaches it in the same request).
 */
function DmAgentSelect({
  linkId,
  integrationId,
  current,
  setDefault,
}: {
  linkId: string;
  integrationId: string;
  current: string | null | undefined;
  setDefault: ReturnType<typeof useSetDmDefault>;
}) {
  const options = useDmDefaultOptions(integrationId);
  const selectId = `dm-${linkId}`;
  const bot = options.data?.shared_bot;
  const items = options.data?.items ?? [];
  return (
    <div className="max-w-md space-y-1.5">
      <Field
        id={selectId}
        label="Agent that answers direct messages"
        hint={bot ? `Choosing an agent that isn't on @${bot.handle} yet adds it there.` : undefined}
      >
        <Select
          id={selectId}
          value={current ?? ""}
          disabled={setDefault.isPending || options.isPending || !bot}
          onChange={(v) => v && setDefault.mutate({ integration_id: integrationId, agent_id: v })}
        >
          <option value="">{current ? "Select…" : "None chosen"}</option>
          {items.map((a) => (
            <option key={a.agent_id} value={a.agent_id}>
              {`${a.name} (@${a.handle}) · ${a.workspace_name}${a.attached ? "" : ` · will be added to @${bot?.handle ?? ""}`}`}
            </option>
          ))}
        </Select>
      </Field>
      {options.isError ? (
        <p className="text-sm text-ink-2">The agents could not be loaded. Reload the page to try again.</p>
      ) : null}
      {options.data && !bot ? (
        <p className="text-sm text-ink-2">This chat platform has no shared bot to message directly.</p>
      ) : null}
    </div>
  );
}

export function AccountChannelsView() {
  const links = useMyChannelLinks();
  const defaults = useDmDefaults();
  const unlink = useUnlinkMine();
  const setDefault = useSetDmDefault();
  const { agents } = useCandidateAgents();

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
                  <DmAgentSelect
                    linkId={l.id}
                    integrationId={l.integration_id}
                    current={d?.agent_id}
                    setDefault={setDefault}
                  />
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
