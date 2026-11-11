"use client";

import { useParams, useRouter } from "next/navigation";

import { AgentFreeze } from "@/components/freeze/freeze-sections";
import { ErrorState, Loading } from "@/components/states";
import { Avatar } from "@/components/agent-avatar";
import { Badge } from "@/components/ui/badge";
import { PageHeader } from "@/components/ui/page-header";
import { StatusDot } from "@/components/ui/status-dot";
import { Tabs, tabId, tabPanelId } from "@/components/ui/tabs";
import { useSandbox } from "@/lib/api/conversations";
import { cn } from "@/lib/utils";
import { useAgents, useResolve, useWorkspace } from "@/lib/api/queries";
import { useTabParam } from "@/lib/use-tab-param";

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
import { PreviewsTab } from "./previews-tab";
import { useInstance, useOrg } from "@/lib/api/queries";

type AgentTab = "chat" | "instructions" | "access" | "connect" | "computer" | "previews" | "memory" | "skills" | "schedules" | "webhooks" | "peers" | "channels";
const AGENT_TABS: readonly AgentTab[] = ["chat", "instructions", "access", "connect", "computer", "previews", "memory", "skills", "schedules", "webhooks", "peers", "channels"];

/** Sandbox state of a hosted agent (existing sandbox endpoint); hidden while unknown or on error. */
function SandboxPill({ agentId }: { agentId: string }) {
  const q = useSandbox(agentId);
  if (!q.data) return null;
  const s = q.data.state;
  const tone = s === "running" ? "accent" : s === "failed" ? "danger" : s === "starting" || s === "stopping" ? "pending" : "neutral";
  // (FIX-025, D-158) A failed start is never shown as "starting" forever: the pill says failed and
  // carries the reason as the title (tooltip); the Computer tab holds the full detail.
  const title =
    s === "failed" && (q.data.failure_message || q.data.failure_code)
      ? `${q.data.failure_message ?? ""}${q.data.failure_code ? ` (${q.data.failure_code})` : ""}`.trim()
      : undefined;
  const label = s === "starting" ? "Sandbox starting" : `Sandbox ${s}`;
  return (
    <StatusDot tone={tone} title={title}>
      {label}
    </StatusDot>
  );
}

export function AgentPage() {
  const { org, ws, handle } = useParams<{ org: string; ws: string; handle: string }>();
  const router = useRouter();
  const resolved = useResolve(org, ws);
  const wsId = resolved.data?.workspace?.id;
  const agents = useAgents(wsId);
  const workspace = useWorkspace(wsId);
  // (P4-PRV-1) The Previews tab exists only for hosted agents when the instance runs previews AND
  // the organization has switched Port previews on (docs/14 §Previews): without the org switch the
  // API answers 409 and the Computer tab shows no Share action.
  const instance = useInstance();
  const orgQ = useOrg(resolved.data?.org.id);
  const previewsEnabled = instance.data?.features.previews === true && orgQ.data?.settings.previews_enabled === true;
  // A deep link to a conversation (`?c=<id>`, D-102) opens the Chat tab. `?tab=` selects a tab (C-506: the chat error links use `?tab=access` and `?tab=instructions`); an unknown or
  // unavailable value (a BYO agent has no Chat) falls back to Instructions.
  const [requestedTab, setTab] = useTabParam<AgentTab>(AGENT_TABS, "instructions", {
    initial: (p) => (p.has("c") ? "chat" : null),
    drop: (t) => (t === "chat" ? [] : ["c"]),
  });

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
          ...(previewsEnabled ? [{ id: "previews" as const, label: "Previews" }] : []),
        ]),
    { id: "access" as const, label: "Access" },
    ...(byo
      ? [{ id: "connect" as const, label: "Computer: Connect" }]
      : [{ id: "computer" as const, label: "Computer" }]),
  ];

  const tab: AgentTab = tabs.some((t) => t.id === requestedTab) ? requestedTab : "instructions";
  // The Chat tab is a fixed-height panel that must fit a 1280×800 window with its composer pinned, so
  // it gets the compact header (one title line, Freeze in the overflow menu). Other tabs keep the full one.
  const compact = tab === "chat" && !byo;
  const freezeProps = workspace.data
    ? {
        agentId: agent.id,
        name: agent.name,
        orgId: workspace.data.org_id,
        frozen: agent.frozen,
        canOwn: workspace.data.my_role === "owner",
      }
    : null;
  // A frozen agent shows its banner (full width) under the header; the menu only offers Freeze.
  const menuFreeze = compact && freezeProps && freezeProps.canOwn && !agent.frozen;

  return (
    <div className={cn("space-y-4", compact ? "space-y-2" : "max-w-4xl")}>
      <PageHeader
        compact={compact}
        breadcrumb={[{ label: "Agents", href: `/o/${org}/w/${ws}/agents` }, { label: agent.name }]}
        title={
          <span className="flex min-w-0 items-center gap-3">
            <Avatar name={agent.name} url={agent.avatar_url} tintKey={agent.id} size={compact ? 32 : 44} />
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
        actions={menuFreeze ? <AgentFreeze {...freezeProps} inMenu /> : undefined}
        className="mb-2"
      />
      {freezeProps && !menuFreeze ? <AgentFreeze {...freezeProps} /> : null}
      <Tabs
        items={tabs.map((t) => ({ value: t.id, label: t.label }))}
        value={tab}
        onValueChange={(v) => setTab(v as AgentTab)}
        variant="underline"
        label="Agent"
        idBase="agent"
      />
      <div role="tabpanel" id={tabPanelId("agent", tab)} aria-labelledby={tabId("agent", tab)} className="pt-2">
        {tab === "chat" && !byo ? <ChatTab agentId={agent.id} wsId={wsId!} base={`/o/${org}/w/${ws}`} orgId={workspace.data?.org_id} agent={{ id: agent.id, name: agent.name, avatarUrl: agent.avatar_url, brief: agent.brief }} /> : null}
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
        {tab === "previews" && !byo ? (
          <PreviewsTab agentId={agent.id} frozen={agent.frozen !== null} />
        ) : null}
        {tab === "instructions" ? (
          <InstructionsTab key={agent.updated_at} agent={agent} wsId={wsId!} />
        ) : null}
        {tab === "access" && workspace.data ? (
          <AccessTab agentId={agent.id} wsId={wsId!} orgId={workspace.data.org_id} canOwn={workspace.data.my_role === "owner"} hosted={!byo} base={`/o/${org}/w/${ws}`} />
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
