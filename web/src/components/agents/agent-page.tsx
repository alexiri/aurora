"use client";

import Link from "next/link";
import { useParams, useRouter } from "next/navigation";
import { useEffect, useRef, useState } from "react";

import { AgentAttentionChip } from "@/components/accounts/attention";
import { AgentFreeze } from "@/components/freeze/freeze-sections";
import { ErrorState, Loading } from "@/components/states";
import { Avatar } from "@/components/agent-avatar";
import { Badge } from "@/components/ui/badge";
import { Altitude, AltitudeScope, MoreControlsLine } from "@/components/ui/altitude";
import { Button } from "@/components/ui/button";
import { useAltitude, useHiddenControls } from "@/lib/altitude";
import { ChevronLeftIcon, SettingsIcon } from "@/components/ui/icons";
import { Tabs, tabId, tabPanelId } from "@/components/ui/tabs";
import { useCanApprove } from "@/lib/api/approvals";
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

import { EvalsTab } from "./evals/evals-tab";
import { FilesTab } from "./files-tab";
import { InstructionsTab } from "./instructions-tab";
import { PreviewsTab } from "./previews-tab";
import { AGENT_PAGE_IDS, availableSections, hiddenPageCount, resolvePage, SECTION_OF, type AgentPageId, type PageAllows, type PageAvailability, type SectionId } from "./sections";
import { AgentState } from "./agent-state";
import { SettingsFrame } from "./settings-shell";
import { SshPage } from "./ssh-page";
import { TerminalTab } from "./terminal-tab";
import { useInstance, useOrg } from "@/lib/api/queries";

/** (P5-REFINE-3) Tells the page's scope how many pages the viewer's altitude leaves out of the navigation. */
function HiddenPages({ availability, allows }: { availability: PageAvailability; allows: PageAllows }) {
  useHiddenControls(hiddenPageCount(availability, allows));
  return null;
}

export function AgentPage() {
  const { org, ws, handle } = useParams<{ org: string; ws: string; handle: string }>();
  const altitude = useAltitude();
  const router = useRouter();
  const resolved = useResolve(org, ws);
  const wsId = resolved.data?.workspace?.id;
  const agents = useAgents(wsId);
  const workspace = useWorkspace(wsId);
  const canApprove = useCanApprove(wsId);
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
  const tab = resolvePage(requestedTab, availability);
  // The viewer's altitude leaves pages out of the navigation; the open page stays listed (a direct `?tab=` still opens).
  const sections = availableSections(availability, altitude.allows, tab);
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
  const chatPage = tab === "chat" && !byo;
  // (P5-REFINE-11, D-408) Settings is configuration behind a labelled button, not a tab: on a Settings page no tab is selected and
  // the button shows the accent outline. Pause and Delete are the danger zone at the bottom of the Settings page.
  const isConfig = sectionId === "settings";
  const barSections = sections.filter((s) => s.id !== "settings");
  const triggerClass = isConfig ? "outline outline-2 outline-accent" : "";
  const backPage: AgentPageId = byo ? "connect" : "chat";

  return (
    // The agent page uses the full content width (FIX-129); long-form panels keep a readable width inside themselves.
    <AltitudeScope>
    <div className="min-w-0 space-y-2">
      <HiddenPages availability={availability} allows={altitude.allows} />
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
              {/* Each item keeps its words on one line; at 390 px the items wrap between themselves, never off the screen. */}
              <span className="flex min-w-0 flex-wrap items-center gap-x-2 whitespace-nowrap text-sm text-ink-2 md:shrink-0 md:flex-nowrap">
                <Altitude id="agent.handle" level="expert">
                  <span className="hidden font-mono text-muted md:inline">@{agent.handle}</span>
                </Altitude>
                {/* The kind and Default badges live in the rail's About part; a BYO agent has no rail. */}
                {byo ? <Badge>{agent.kind}</Badge> : null}
                {byo && agent.is_default ? <Badge tone="accent">Default</Badge> : null}
                {byo || !wsId ? null : <AgentState agentId={agent.id} wsId={wsId} canOwn={canOwn} canApprove={canApprove} hosted />}
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
          <Button
            type="button"
            variant="outline"
            size="sm"
            aria-current={isConfig ? "page" : undefined}
            className={`min-h-11 md:min-h-9 ${triggerClass}`}
            onClick={() => choosePage(lastPage.current.settings ?? "instructions")}>
            <SettingsIcon width={16} height={16} />
            Settings
          </Button>
        </div>
        <div className="order-3 w-full min-w-0 md:order-2 md:w-auto md:shrink-0">
          <Tabs
            items={barSections.map((s) => ({ value: s.id, label: s.label }))}
            value={isConfig ? "" : sectionId}
            onValueChange={goSection}
            label="Agent sections"
            idBase="agent-section"
            className="flex w-fit max-md:w-full"
            tabClassName="max-md:min-h-11 max-md:flex-1 max-md:justify-center max-md:px-1.5"
          />
        </div>
      </header>
      {/* The banner of a paused agent, full width under the header (with Resume for owners; members see it read-only). Pause
          itself lives in the Settings danger zone. */}
      {freezeProps && agent.frozen ? <AgentFreeze {...freezeProps} /> : null}
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
      <div
        {...(isConfig ? {} : { role: "tabpanel", id: tabPanelId("agent-section", sectionId), "aria-labelledby": tabId("agent-section", sectionId) })}
        className="space-y-3">
        <SettingsFrame
          shell={
            isConfig
              ? {
                  agentId: agent.id,
                  agentName: agent.name,
                  orgId: workspace.data?.org_id,
                  page: tab,
                  pages: section.pages,
                  onPage: choosePage,
                  onBack: () => choosePage(backPage),
                  backLabel: backPage === "chat" ? "Chat" : "Computer",
                  canOwn,
                  frozen: agent.frozen,
                  onDelete: () => setDeleteOpen(true),
                }
              : null
          }>
        {section.pages.length > 1 && !isConfig ? (
          <Tabs
            items={section.pages.map((p) => ({ value: p.id, label: p.label }))}
            value={tab}
            onValueChange={(v) => choosePage(v as AgentPageId)}
            label={`${section.label} pages`}
            idBase="agent"
            // What does not fit at 390 px goes into a More menu: never a strip that scrolls sideways (docs/14 §Tabs).
            overflow="menu"
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
          {tab === "memory" && !byo ? <MemoryTab agentId={agent.id} agentName={agent.name} memoryLanguage={workspace.data?.memory_language} /> : null}
          {tab === "skills" && !byo && workspace.data ? <SkillsTab agentId={agent.id} orgId={workspace.data.org_id} /> : null}
          {tab === "evals" && !byo ? <EvalsTab agentId={agent.id} frozen={agent.frozen !== null} canOwn={canOwn} defaultTimezone={workspace.data?.timezone} /> : null}
          {tab === "schedules" && !byo ?<SchedulesTab agentId={agent.id} defaultTimezone={workspace.data?.timezone} wsId={workspace.data?.id} canOwn={canOwn} /> : null}
          {tab === "webhooks" && !byo ? <WebhooksTab agentId={agent.id} wsId={workspace.data?.id} canOwn={canOwn} /> : null}
          {tab === "peers" && !byo ? <PeersTab agentId={agent.id} /> : null}
          {tab === "channels" && !byo && workspace.data ? (
            <AgentChannelsTab
              agentId={agent.id}
              agentName={agent.name}
              orgId={workspace.data.org_id}
              canOwn={workspace.data.my_role === "owner"}
              wsId={workspace.data.id}
              narrationCards={agent.narration_cards}
            />
          ) : null}
          {tab === "channels" && !byo && workspace.isPending ? <Loading /> : null}
          {tab === "computer" && !byo ? (
            <ComputerTab agentId={agent.id} agentName={agent.name} workspaceId={workspace.data?.id} frozen={agent.frozen !== null} terminal={terminalEnabled} />
          ) : null}
          {tab === "terminal" && !byo && terminalEnabled ? <TerminalTab agentId={agent.id} frozen={agent.frozen !== null} /> : null}
          {tab === "files" && !byo && terminalEnabled ? <FilesTab agentId={agent.id} frozen={agent.frozen !== null} /> : null}
          {tab === "previews" && !byo ? <PreviewsTab agentId={agent.id} frozen={agent.frozen !== null} /> : null}
          {tab === "ssh" && !byo && sshEnabled ? <SshPage agentId={agent.id} frozen={agent.frozen !== null} /> : null}
          {tab === "instructions" ? <InstructionsTab key={agent.updated_at} agent={agent} wsId={wsId!} /> : null}
          {tab === "access" && workspace.data ? (
            <AccessTab agentId={agent.id} agentName={agent.name} wsId={wsId!} orgId={workspace.data.org_id} canOwn={canOwn} hosted={!byo} base={base} agentBase={`${base}/agents/${agent.handle}`} />
          ) : null}
          {tab === "access" && workspace.isPending ? <Loading /> : null}
          {tab === "connect" && byo ? <ConnectTab agent={agent} wsId={wsId} canOwn={canOwn} /> : null}
        </div>
        </SettingsFrame>
      </div>
      {chatPage ? null : <MoreControlsLine className="pt-2" />}
    </div>
    </AltitudeScope>
  );
}
