"use client";

import { useParams, useRouter } from "next/navigation";
import { useState } from "react";

import { AgentFreeze } from "@/components/freeze/freeze-sections";
import { ErrorState, Loading } from "@/components/states";
import { Avatar } from "@/components/agent-avatar";
import { Badge } from "@/components/ui/badge";
import { PageHeader } from "@/components/ui/page-header";
import { StatusDot } from "@/components/ui/status-dot";
import { Tabs, tabId, tabPanelId } from "@/components/ui/tabs";
import { useSandbox } from "@/lib/api/conversations";
import { useAgents, useResolve, useWorkspace } from "@/lib/api/queries";

import { AccessTab } from "./access-tab";
import { ChatTab } from "./chat-tab";
import { ComputerTab } from "./computer-tab";
import { ConnectTab } from "./connect-tab";
import { DeleteZone } from "./delete-zone";
import { MemoryTab } from "@/components/memory/memory-tab";
import { SkillsTab } from "@/components/skills/skills-tab";
import { AgentChannelsTab } from "@/components/channels/agent-channels-tab";
import { PeersTab } from "@/components/peers/peers-tab";
import { SchedulesTab } from "@/components/schedules/schedules-tab";
import { WebhooksTab } from "@/components/webhooks/webhooks-tab";

import { InstructionsTab } from "./instructions-tab";

/** Sandbox state of a hosted agent (existing sandbox endpoint); hidden while unknown or on error. */
function SandboxPill({ agentId }: { agentId: string }) {
  const q = useSandbox(agentId);
  if (!q.data) return null;
  const s = q.data.state;
  const tone = s === "running" ? "accent" : s === "failed" ? "danger" : s === "starting" || s === "stopping" ? "pending" : "neutral";
  return <StatusDot tone={tone}>Sandbox {s}</StatusDot>;
}

export function AgentPage() {
  const { org, ws, handle } = useParams<{ org: string; ws: string; handle: string }>();
  const router = useRouter();
  const resolved = useResolve(org, ws);
  const wsId = resolved.data?.workspace?.id;
  const agents = useAgents(wsId);
  const workspace = useWorkspace(wsId);
  const [tab, setTab] = useState<"chat" | "instructions" | "access" | "connect" | "computer" | "memory" | "skills" | "schedules" | "webhooks" | "peers" | "channels">("instructions");

  if (resolved.isPending || (wsId && agents.isPending)) return <Loading />;
  if (resolved.isError) return <ErrorState error={resolved.error} onRetry={() => resolved.refetch()} />;
  if (agents.isError) return <ErrorState error={agents.error} onRetry={() => agents.refetch()} />;
  const agent = agents.data?.items.find((a) => a.handle === handle);
  if (!agent) return <ErrorState error={{ code: "not_found" }} />;

  const byo = agent.kind === "byo";
  const tabs = [
    ...(byo ? [] : [{ id: "chat" as const, label: "Chat" }]),
    { id: "instructions" as const, label: "Instructions" },
    ...(byo
      ? []
      : [
          { id: "memory" as const, label: "Memory" },
          { id: "skills" as const, label: "Skills" },
          { id: "schedules" as const, label: "Schedules" },
          { id: "webhooks" as const, label: "Webhooks" },
          { id: "peers" as const, label: "Peers" },
          { id: "channels" as const, label: "Channels" },
        ]),
    { id: "access" as const, label: "Access" },
    ...(byo
      ? [{ id: "connect" as const, label: "Computer: Connect" }]
      : [{ id: "computer" as const, label: "Computer" }]),
  ];

  return (
    <div className="max-w-4xl space-y-4">
      <PageHeader
        breadcrumb={[{ label: "Agents", href: `/o/${org}/w/${ws}/agents` }, { label: agent.name }]}
        title={
          <span className="flex min-w-0 items-center gap-3">
            <Avatar name={agent.name} url={agent.avatar_url} tintKey={agent.id} size={48} />
            <span className="min-w-0 truncate">{agent.name}</span>
          </span>
        }
        description={
          <span className="flex flex-wrap items-center gap-2">
            <span className="font-mono text-muted">@{agent.handle}</span>
            <Badge>{agent.kind}</Badge>
            {agent.is_default ? <Badge tone="accent">Default</Badge> : null}
            {byo ? null : <SandboxPill agentId={agent.id} />}
          </span>
        }
        className="mb-2"
      />
      {workspace.data ? (
        <AgentFreeze
          agentId={agent.id}
          name={agent.name}
          orgId={workspace.data.org_id}
          frozen={agent.frozen}
          canOwn={workspace.data.my_role === "owner"}
        />
      ) : null}
      <Tabs
        items={tabs.map((t) => ({ value: t.id, label: t.label }))}
        value={tab}
        onValueChange={(v) => setTab(v as typeof tab)}
        variant="underline"
        label="Agent"
        idBase="agent"
      />
      <div role="tabpanel" id={tabPanelId("agent", tab)} aria-labelledby={tabId("agent", tab)} className="pt-2">
        {tab === "chat" && !byo ? <ChatTab agentId={agent.id} wsId={wsId!} base={`/o/${org}/w/${ws}`} orgId={workspace.data?.org_id} agent={{ id: agent.id, name: agent.name, avatarUrl: agent.avatar_url }} /> : null}
        {tab === "memory" && !byo ? <MemoryTab agentId={agent.id} memoryLanguage={workspace.data?.memory_language} /> : null}
        {tab === "skills" && !byo && workspace.data ? <SkillsTab agentId={agent.id} orgId={workspace.data.org_id} /> : null}
        {tab === "schedules" && !byo ? <SchedulesTab agentId={agent.id} defaultTimezone={workspace.data?.timezone} wsId={workspace.data?.id} canOwn={workspace.data?.my_role === "owner"} /> : null}
        {tab === "webhooks" && !byo ? <WebhooksTab agentId={agent.id} wsId={workspace.data?.id} canOwn={workspace.data?.my_role === "owner"} /> : null}
        {tab === "peers" && !byo ? <PeersTab agentId={agent.id} /> : null}
        {tab === "channels" && !byo && workspace.data ? (
          <AgentChannelsTab agentId={agent.id} orgId={workspace.data.org_id} canOwn={workspace.data.my_role === "owner"} />
        ) : null}
        {tab === "channels" && !byo && workspace.isPending ? <Loading /> : null}
        {tab === "computer" && !byo ? <ComputerTab agentId={agent.id} /> : null}
        {tab === "instructions" ? (
          <InstructionsTab key={agent.updated_at} agent={agent} wsId={wsId!} />
        ) : null}
        {tab === "access" && workspace.data ? (
          <AccessTab agentId={agent.id} wsId={wsId!} orgId={workspace.data.org_id} canOwn={workspace.data.my_role === "owner"} />
        ) : null}
        {tab === "access" && workspace.isPending ? <Loading /> : null}
        {tab === "connect" && byo ? (
          <ConnectTab agent={agent} wsId={wsId} canOwn={workspace.data?.my_role === "owner"} />
        ) : null}
        {tab === "instructions" ? (
          <div className="pt-6">
            <DeleteZone agentId={agent.id} handle={agent.handle} wsId={wsId!} onDeleted={() => router.replace(`/o/${org}/w/${ws}/agents`)} />
          </div>
        ) : null}
      </div>
    </div>
  );
}
