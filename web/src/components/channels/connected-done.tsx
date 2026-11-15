"use client";

import Link from "next/link";
import { useState } from "react";

import { Button, buttonVariants } from "@/components/ui/button";
import { Field, inputClass } from "@/components/ui/field";
import { Notice } from "@/components/ui/notice";
import { useAttachPresence, type ChannelIntegration } from "@/lib/api/channels";
import type { ConnectorInfo } from "@/lib/api/connectors";
import { useAgents, useWorkspaces } from "@/lib/api/queries";
import { cn } from "@/lib/utils";

import { MutationError } from "./shared";

/** `https://<server>/<team>/messages/@<bot>`, or null when the server is not an http(s) address. */
export function dmUrl(server: string | undefined, team: string | undefined, bot: string | undefined): string | null {
  if (!server || !team || !bot) return null;
  try {
    const u = new URL(server);
    if (u.protocol !== "https:" && u.protocol !== "http:") return null;
    const base = `${u.origin}${u.pathname.replace(/\/+$/, "")}`;
    return `${base}/${encodeURIComponent(team)}/messages/@${encodeURIComponent(bot.replace(/^@/, ""))}`;
  } catch {
    return null;
  }
}

function AttachAgent({ integrationId, agentId, botLabel }: { integrationId: string; agentId: string; botLabel: string }) {
  const attach = useAttachPresence(agentId);
  if (attach.isSuccess) {
    return (
      <Notice tone="accent" role="status">
        Attached. People can now talk to this agent through {botLabel}.
      </Notice>
    );
  }
  return (
    <div className="space-y-2">
      <Button disabled={attach.isPending} onClick={() => attach.mutate({ integration_id: integrationId, mode: "shared" })}>
        {attach.isPending ? "Attaching…" : "Attach with the shared bot"}
      </Button>
      <MutationError error={attach.error} />
    </div>
  );
}

function AttachPicker({ orgId, integrationId, botLabel }: { orgId: string; integrationId: string; botLabel: string }) {
  const workspaces = useWorkspaces(orgId);
  const [ws, setWs] = useState("");
  const [agent, setAgent] = useState("");
  const list = workspaces.data?.items ?? [];
  const wsId = ws || list[0]?.id || "";
  const agents = useAgents(wsId || undefined);
  if (workspaces.isSuccess && list.length === 0) {
    return <p className="text-sm text-muted">There is no workspace yet. Create one, then attach an agent from its Channels tab.</p>;
  }
  const items = agents.data?.items ?? [];
  return (
    <div className="space-y-3">
      <Field id="done-ws" label="Workspace">
        <select
          id="done-ws"
          className={inputClass}
          value={wsId}
          onChange={(e) => {
            setWs(e.target.value);
            setAgent("");
          }}
        >
          {list.map((w) => (
            <option key={w.id} value={w.id}>
              {w.name}
            </option>
          ))}
        </select>
      </Field>
      <Field id="done-agent" label="Agent">
        <select id="done-agent" className={inputClass} value={agent} onChange={(e) => setAgent(e.target.value)}>
          <option value="">Select…</option>
          {items.map((a) => (
            <option key={a.id} value={a.id}>
              {a.name}
            </option>
          ))}
        </select>
      </Field>
      {agents.isSuccess && items.length === 0 ? (
        <p className="text-sm text-muted">This workspace has no agents yet.</p>
      ) : null}
      {agent ? <AttachAgent key={agent} integrationId={integrationId} agentId={agent} botLabel={botLabel} /> : null}
    </div>
  );
}

/**
 * (FIX-127) The done screen of the integration stepper: who the bot is and three next steps,
 * each one a button — attach an agent (shared presence), link the admin's own chat account, and
 * open a direct message with the bot.
 */
export function ConnectedDone({
  orgId,
  connector,
  integration,
  values,
  choices,
  onClose,
}: {
  orgId: string;
  connector: ConnectorInfo;
  integration: ChannelIntegration;
  values: Record<string, string>;
  choices: Record<string, { value: string; label: string }[]>;
  onClose: () => void;
}) {
  const bot = integration.shared_bot?.handle?.replace(/^@/, "");
  const teamValue = values.team;
  const teamLabel = choices.team?.find((c) => c.value === teamValue)?.label ?? teamValue;
  const dm = dmUrl(values.server_url, teamValue, bot);
  const botLabel = bot ? `@${bot}` : "the shared bot";
  return (
    <div className="space-y-5" data-testid="connected-done">
      <Notice tone="accent" role="status">
        {connector.display_name} is connected as <strong>{botLabel}</strong>
        {teamLabel ? <> in {teamLabel}</> : null}.
      </Notice>
      <section aria-labelledby="done-attach" className="space-y-2">
        <h3 id="done-attach" className="text-sm font-semibold text-ink">
          1. Attach an agent
        </h3>
        <AttachPicker orgId={orgId} integrationId={integration.id} botLabel={botLabel} />
      </section>
      <section aria-labelledby="done-link" className="space-y-2">
        <h3 id="done-link" className="text-sm font-semibold text-ink">
          2. Link your {connector.display_name} account
        </h3>
        <p className="text-sm text-ink-2">So the agent knows it is you when you talk to it.</p>
        <Link href="/account/channels" className={cn(buttonVariants({ variant: "outline" }))}>
          Link your {connector.display_name} account
        </Link>
      </section>
      <section aria-labelledby="done-dm" className="space-y-2">
        <h3 id="done-dm" className="text-sm font-semibold text-ink">
          3. Say hello
        </h3>
        {dm ? (
          <a href={dm} target="_blank" rel="noopener noreferrer" className={cn(buttonVariants({ variant: "outline" }))}>
            Open a DM with {botLabel}
          </a>
        ) : (
          <p className="text-sm text-muted">Open a direct message with {botLabel} in {connector.display_name}.</p>
        )}
      </section>
      <Button variant="ghost" onClick={onClose}>
        Done
      </Button>
    </div>
  );
}
