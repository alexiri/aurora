"use client";

import Link from "next/link";
import { useParams, useRouter } from "next/navigation";
import { useEffect, useRef, useState } from "react";

import { AgentAttentionChip } from "@/components/accounts/attention";
import { AgentFreeze } from "@/components/freeze/freeze-sections";
import { ErrorState, Loading } from "@/components/states";
import { Avatar } from "@/components/agent-avatar";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ChevronLeftIcon, MoreIcon } from "@/components/ui/icons";
import { Menu, MenuContent, MenuItem, MenuTrigger } from "@/components/ui/menu";
import { StatusDot } from "@/components/ui/status-dot";
import { Tabs, tabId, tabPanelId } from "@/components/ui/tabs";
import { useSandbox } from "@/lib/api/conversations";
import { useAgents, useResolve, useWorkspace } from "@/lib/api/queries";
import { useTabParam } from "@/lib/use-tab-param";

import { AccessTab } from "./access-tab";
import { ChatTab } from "./chat-tab";
import { ComputerTab } from "./computer-tab";
import { ConnectTab } from "./connect-tab";
import { DeleteAgentDialog } from "./delete-zone";
import { MemoryTab } from "@/components/memory/memory-tab";
import { SkillsTab } from "@/components/skills/skills-tab";
import { AgentChannelsTab } from "@/components/channels/agent-channels-tab";
import { PeersTab } from "@/components/peers/peers-tab";
import { SchedulesTab } from "@/components/schedules/schedules-tab";
import { WebhooksTab } from "@/components/webhooks/webhooks-tab";

import { FilesTab } from "./files-tab";
import { InstructionsTab } from "./instructions-tab";
import { PreviewsTab } from "./previews-tab";
import { AGENT_PAGE_IDS, availableSections, resolvePage, SECTION_OF, type AgentPageId, type SectionId } from "./sections";
import { computerWord, sandboxLook } from "./status-rail/status-data";
import { SshPage } from "./ssh-page";
import { TerminalTab } from "./terminal-tab";
import { useInstance, useOrg } from "@/lib/api/queries";

/** Sandbox state of a hosted agent (existing sandbox endpoint); hidden while unknown or on error. */
function SandboxPill({ agentId }: { agentId: string }) {
  const q = useSandbox(agentId);
  const [showReason, setShowReason] = useState(false);
  if (!q.data) return null;
  // (FIX-025, D-158) A failed start is never shown as "starting" forever: the pill says failed and
  // carries the reason as the title (tooltip); the Computer tab holds the full detail.
  const look = sandboxLook(q.data);
  const dot = (
    <StatusDot tone={look.tone} title={look.title}>
      Computer: {computerWord(q.data.state)}
    </StatusDot>
  );
  // (P5-UX-1) The state in words; a reason (a failed start) shows on hover and, for a touch screen, on tap.
  if (!look.title) return dot;
  return (
    <span className="inline-flex flex-col gap-1">
      <button type="button" aria-expanded={showReason} onClick={() => setShowReason((v) => !v)} className="inline-flex rounded-control text-left focus-visible:outline focus-visible:outline-2 focus-visible:outline-accent">
        {dot}
      </button>
      {showReason ? (
        <span role="status" className="max-w-sm text-xs text-ink-2">
          {look.title}
        </span>
      ) : null}
    </span>
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
  // (P4-TERM, C-622) The Terminal tab exists only for hosted agents while the instance's
  // `terminal.enabled` is on. `can_use` is the tab's own authority (the API re-checks it on
  // every open; a caller without it gets 403/404 there and this tab's notices say so).
  const terminalEnabled = instance.data?.features.terminal === true;
  // (P4-SSH-1, C-795) The SSH page: hosted agents while `ssh.enabled` is on (independent of the terminal).
  const sshEnabled = instance.data?.features.ssh === true;
  // A deep link to a conversation (`?c=<id>`, D-102) opens the Chat page. `?tab=` selects a page by its id (C-506: the
  // chat error links use `?tab=access` and `?tab=instructions`); an unknown or unavailable value falls back to the
  // agent's default page (Chat for a hosted agent, Instructions for a BYO one, D-200).
  const [requestedTab, setTab] = useTabParam<AgentPageId>(AGENT_PAGE_IDS, "chat", {
    initial: (p) => (p.has("c") ? "chat" : null),
    // `c` belongs to Chat and `path` to Files (P5-FILES-2): leaving a page takes its own parameter along.
    drop: (t) => [...(t === "chat" ? [] : ["c"]), ...(t === "files" ? [] : ["path"])],
  });
  const [deleteOpen, setDeleteOpen] = useState(false);
  const [statusOpen, setStatusOpen] = useState(false);
  // The page last open in each section, so a section's switcher button returns to it.
  const lastPage = useRef<Partial<Record<SectionId, AgentPageId>>>({});

  // (FIX-153, C-894) The segment may be the agent's id (the gateway's Access-tab link carries one): replace it with
  // the handle, keeping the query (`?tab=access`). An unknown value is the not-found state below.
  const byId = agents.data?.items.find((a) => a.id === handle && a.handle !== handle);
  const byIdHandle = byId?.handle;
  useEffect(() => {
    if (byIdHandle) router.replace(`/o/${org}/w/${ws}/agents/${byIdHandle}${window.location.search}`);
  }, [byIdHandle, org, ws, router]);

  if (resolved.isPending || (wsId && agents.isPending)) return <Loading />;
  if (resolved.isError) return <ErrorState error={resolved.error} onRetry={() => resolved.refetch()} />;
  if (agents.isError) return <ErrorState error={agents.error} onRetry={() => agents.refetch()} />;
  const agent = agents.data?.items.find((a) => a.handle === handle);
  if (!agent && byId) return <Loading />;
  if (!agent) return <ErrorState error={{ code: "not_found" }} />;

  const byo = agent.kind === "byo";
  const availability = { byo, previews: previewsEnabled, terminal: terminalEnabled, ssh: sshEnabled };
  const sections = availableSections(availability);
  const tab = resolvePage(requestedTab, availability);
  const sectionId = SECTION_OF[tab];
  const section = sections.find((s) => s.id === sectionId)!;
  const choosePage = (p: AgentPageId) => {
    lastPage.current[SECTION_OF[p]] = p;
    setTab(p);
  };
  const goSection = (id: string) => {
    const next = sections.find((s) => s.id === id);
    if (!next) return;
    const remembered = lastPage.current[next.id];
    choosePage(remembered && next.pages.some((p) => p.id === remembered) ? remembered : next.pages[0]!.id);
  };
  const base = `/o/${org}/w/${ws}`;
  const canOwn = workspace.data?.my_role === "owner";
  const freezeProps = workspace.data
    ? {
        agentId: agent.id,
        name: agent.name,
        orgId: workspace.data.org_id,
        frozen: agent.frozen,
        canOwn,
      }
    : null;
  // Agent actions (D-200): Freeze (with its confirm dialog) and Delete agent…, for `can_own`. A frozen agent's banner
  // (with Unfreeze) stays under the header; the menu then only offers Delete.
  const deleteItem = (
    <MenuItem className="text-danger-ink data-[highlighted]:bg-danger-soft" onSelect={() => setDeleteOpen(true)}>
      Delete agent…
    </MenuItem>
  );
  const freezeInMenu = !!freezeProps && canOwn && !agent.frozen;
  const deleteOnlyMenu = !!freezeProps && canOwn && !!agent.frozen;
  const chatPage = tab === "chat" && !byo;

  return (
    // The agent page uses the full content width (FIX-129); long-form panels keep a readable width inside themselves.
    <div className="min-w-0 space-y-2">
      {/* One line at md and up (breadcrumb above the name beside the avatar · section switcher · Agent actions); on a phone
          one compact row (back, avatar, name + status, actions) with the section switcher as the row under it (14 §Agent page). */}
      <header className="flex flex-wrap items-center gap-x-3 gap-y-1 md:flex-nowrap md:gap-x-4">
        <div className="order-1 flex min-w-0 flex-1 items-center gap-2 md:gap-3">
          <Link href={`${base}/agents`} aria-label="Back to agents" className="-ml-2 inline-flex size-11 shrink-0 items-center justify-center rounded-control text-ink-2 hover:text-ink md:hidden">
            <ChevronLeftIcon className="size-5" />
          </Link>
          <span className="shrink-0 md:hidden">
            <Avatar name={agent.name} url={agent.avatar_url} tintKey={agent.id} size={32} />
          </span>
          <span className="hidden shrink-0 md:block">
            <Avatar name={agent.name} url={agent.avatar_url} tintKey={agent.id} size={40} />
          </span>
          <div className="min-w-0">
            <nav aria-label="Breadcrumb" className="hidden text-xs leading-4 text-muted md:block">
              <Link href={`${base}/agents`} className="hover:text-ink hover:underline">
                Agents
              </Link>{" "}
              <span aria-hidden="true">/</span>
            </nav>
            <div className="flex min-w-0 flex-col md:flex-row md:items-center md:gap-x-3">
              <h1 className="min-w-0 truncate font-display text-base font-semibold leading-5 text-ink md:text-xl md:leading-7">{agent.name}</h1>
              <span className="flex items-center gap-2 text-sm text-ink-2">
                <span className="hidden font-mono text-muted md:inline">@{agent.handle}</span>
                {/* The kind and Default badges live in the rail's About part; a BYO agent has no rail. */}
                {byo ? <Badge>{agent.kind}</Badge> : null}
                {byo && agent.is_default ? <Badge tone="accent">Default</Badge> : null}
                {byo ? null : <SandboxPill agentId={agent.id} />}
                {wsId ? <AgentAttentionChip workspaceId={wsId} agentId={agent.id} accessHref={`${base}/agents/${agent.handle}?tab=access`} /> : null}
              </span>
            </div>
          </div>
        </div>
        <div className="order-2 flex shrink-0 items-center gap-2 md:order-3">
          {chatPage ? (
            <Button type="button" variant="outline" size="sm" className="min-h-11 md:hidden" onClick={() => setStatusOpen(true)}>
              Status
            </Button>
          ) : null}
          {freezeInMenu && freezeProps ? <AgentFreeze {...freezeProps} inMenu menuExtra={deleteItem} /> : null}
          {deleteOnlyMenu ? (
            <Menu modal={false}>
              <MenuTrigger asChild>
                <Button variant="outline" size="sm" aria-label="Agent actions" className="size-11 min-h-11 px-0 md:size-9 md:min-h-9">
                  <MoreIcon width={20} height={20} />
                </Button>
              </MenuTrigger>
              <MenuContent align="end">{deleteItem}</MenuContent>
            </Menu>
          ) : null}
        </div>
        <div className="order-3 w-full min-w-0 md:order-2 md:w-108 md:shrink">
          <Tabs
            items={sections.map((s) => ({ value: s.id, label: s.label }))}
            value={sectionId}
            onValueChange={goSection}
            label="Agent sections"
            idBase="agent-section"
            overflow="menu"
            tabClassName="max-md:min-h-11 max-md:px-1.5"
          />
        </div>
      </header>
      {/* The banner of a frozen agent, full width under the header (and the non-owner's read-only copy of it). */}
      {freezeProps && !freezeInMenu ? <AgentFreeze {...freezeProps} /> : null}
      {freezeProps && canOwn ? (
        <DeleteAgentDialog
          agentId={agent.id}
          handle={agent.handle}
          wsId={wsId!}
          open={deleteOpen}
          onOpenChange={setDeleteOpen}
          onDeleted={() => router.replace(`${base}/agents`)}
        />
      ) : null}
      <div role="tabpanel" id={tabPanelId("agent-section", sectionId)} aria-labelledby={tabId("agent-section", sectionId)} className="space-y-3">
        {section.pages.length > 1 ? (
          <Tabs
            items={section.pages.map((p) => ({ value: p.id, label: p.label }))}
            value={tab}
            onValueChange={(v) => choosePage(v as AgentPageId)}
            label={`${section.label} pages`}
            idBase="agent"
          />
        ) : null}
        <div
          {...(section.pages.length > 1 ? { role: "tabpanel", id: tabPanelId("agent", tab), "aria-labelledby": tabId("agent", tab) } : {})}
          className="pt-1"
        >
          {tab === "chat" && !byo ? (
            <ChatTab
              agentId={agent.id}
              wsId={wsId!}
              base={base}
              orgId={workspace.data?.org_id}
              agent={{ id: agent.id, name: agent.name, handle: agent.handle, avatarUrl: agent.avatar_url, brief: agent.brief, frozen: agent.frozen !== null }}
              agentKind={agent.kind}
              isDefault={agent.is_default}
              terminalAvailable={terminalEnabled}
              sshAvailable={sshEnabled && !byo}
              onNavigate={choosePage}
              statusSheetOpen={statusOpen}
              onStatusSheetOpenChange={setStatusOpen}
            />
          ) : null}
          {tab === "memory" && !byo ? <MemoryTab agentId={agent.id} memoryLanguage={workspace.data?.memory_language} /> : null}
          {tab === "skills" && !byo && workspace.data ? <SkillsTab agentId={agent.id} orgId={workspace.data.org_id} /> : null}
          {tab === "schedules" && !byo ? <SchedulesTab agentId={agent.id} defaultTimezone={workspace.data?.timezone} wsId={workspace.data?.id} canOwn={canOwn} /> : null}
          {tab === "webhooks" && !byo ? <WebhooksTab agentId={agent.id} wsId={workspace.data?.id} canOwn={canOwn} /> : null}
          {tab === "peers" && !byo ? <PeersTab agentId={agent.id} /> : null}
          {tab === "channels" && !byo && workspace.data ? (
            <AgentChannelsTab
              agentId={agent.id}
              orgId={workspace.data.org_id}
              canOwn={workspace.data.my_role === "owner"}
              wsId={workspace.data.id}
              narrationCards={agent.narration_cards}
            />
          ) : null}
          {tab === "channels" && !byo && workspace.isPending ? <Loading /> : null}
          {tab === "computer" && !byo ? (
            <ComputerTab agentId={agent.id} frozen={agent.frozen !== null} terminal={terminalEnabled} />
          ) : null}
          {tab === "terminal" && !byo && terminalEnabled ? <TerminalTab agentId={agent.id} frozen={agent.frozen !== null} /> : null}
          {tab === "files" && !byo && terminalEnabled ? <FilesTab agentId={agent.id} frozen={agent.frozen !== null} /> : null}
          {tab === "previews" && !byo ? <PreviewsTab agentId={agent.id} frozen={agent.frozen !== null} /> : null}
          {tab === "ssh" && !byo && sshEnabled ? <SshPage agentId={agent.id} frozen={agent.frozen !== null} /> : null}
          {tab === "instructions" ? <InstructionsTab key={agent.updated_at} agent={agent} wsId={wsId!} /> : null}
          {tab === "access" && workspace.data ? (
            <AccessTab agentId={agent.id} agentName={agent.name} wsId={wsId!} orgId={workspace.data.org_id} canOwn={canOwn} hosted={!byo} base={base} />
          ) : null}
          {tab === "access" && workspace.isPending ? <Loading /> : null}
          {tab === "connect" && byo ? <ConnectTab agent={agent} wsId={wsId} canOwn={canOwn} /> : null}
        </div>
      </div>
    </div>
  );
}
