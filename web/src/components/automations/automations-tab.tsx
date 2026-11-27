"use client";

import { useRef, useState } from "react";

import { AutomationsBulkBar, type BulkAutomation } from "@/components/automations/automations-bulk";
import { RowMenu, RowShell } from "@/components/automations/row-parts";
import { ScheduleForm } from "@/components/automations/schedule-form";
import { ScheduleRow } from "@/components/automations/schedule-row";
import { WebhookForm, type WebhookReveal } from "@/components/automations/webhook-form";
import { WebhookRevealDialog } from "@/components/automations/webhook-reveal";
import { WebhookRow } from "@/components/automations/webhook-row";
import { ChannelsBody, NoChatYet } from "@/components/channels/agent-channels-tab";
import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Altitude } from "@/components/ui/altitude";
import { BulkSelectAll, useBulkOn, useSelection } from "@/components/ui/bulk-bar";
import { Button } from "@/components/ui/button";
import { ConfirmDialog } from "@/components/ui/confirm-dialog";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import { MenuItem, MenuSeparator } from "@/components/ui/menu";
import { ErrorNotice } from "@/components/ui/notice";
import { ClockIcon, LinkIcon, MessageIcon, PlusIcon } from "@/components/ui/icons";
import { ItemList, Section } from "@/components/ui/section";
import { Segmented } from "@/components/ui/segmented";
import { type AutomationFilter } from "@/components/agents/sections";
import { useDetachPresence, usePresenceOptions, usePresences, type Presence } from "@/lib/api/channels";
import { useConnectors } from "@/lib/api/connectors";
import { useOrg } from "@/lib/api/queries";
import { useSchedules } from "@/lib/api/schedules";
import { useWebhooks } from "@/lib/api/webhooks";
import { useWhoCan } from "@/lib/api/who-can";
import { useVerb } from "@/lib/palette-verb";
import { WhoCan } from "@/components/ui/who-can";

type NewKind = "schedule" | "webhook" | "chat";

/** (P5-REFINE-14) A chat the agent is in, as a row: the platform, where, On or Paused; the setup is in the Chats group below. */
function ChatRow({ agentId, agentName, p, platform }: { agentId: string; agentName: string; p: Presence; platform: string }) {
  const on = p.status === "active" || p.status === "needs_attention";
  const detach = useDetachPresence(agentId);
  const [removing, setRemoving] = useState(false);
  const openChannel = () => document.getElementById("chats")?.scrollIntoView({ block: "start" });
  return (
    <RowShell
      kind="chat"
      name={`${platform} · ${p.tenant_label}`}
      on={on}
      when={on ? `In chats on ${platform}` : `In chats on ${platform} · not answering yet`}
      notices={
        <>
          {p.status_reason ? <p className="text-sm text-ink-2">{p.status_reason}</p> : null}
          <ErrorNotice error={detach.error} />
        </>
      }
      actions={
        <>
          <Button size="sm" variant="outline" onClick={openChannel}>
            Open channel
          </Button>
          <RowMenu name={`${platform} · ${p.tenant_label}`}>
            <MenuItem onSelect={openChannel}>Open channel</MenuItem>
            <MenuSeparator />
            <MenuItem className="text-danger-ink" onSelect={() => setRemoving(true)}>
              Remove from channel…
            </MenuItem>
          </RowMenu>
          <ConfirmDialog
            open={removing}
            onOpenChange={setRemoving}
            title="Remove from channel?"
            description={`${agentName} stops answering in ${p.tenant_label} (${p.mode} identity). Conversations stay; you can add it again later.`}
            confirmLabel="Remove from channel"
            danger
            busy={detach.isPending}
            onConfirm={() => detach.mutate(p.id, { onSettled: () => setRemoving(false) })}
          />
        </>
      }
    />
  );
}

/**
 * (P5-REFINE-14, D-405, rec 12) **Automations**: when the agent runs. One list of schedules, webhooks and chats with a filter, one
 * header button (**New automation**, which asks the kind first), and the Chats set-up (presences, space bindings, reach
 * requests, progress in chat) under the list. The old `?tab=schedules|webhooks|channels` open this page filtered to their kind.
 */
export function AutomationsTab({
  agentId,
  agentName,
  agentBase,
  orgId,
  wsId,
  canOwn,
  defaultTimezone,
  workspaceName,
  narrationCards,
  initialFilter = "all",
  agentFrozen = false,
  readOnly = false,
}: {
  /**
   * (FIX-277) Emergency read access: the schedules and webhooks with their runs and deliveries, nothing to add or
   * change, and no chats (the grant doesn't cover them).
   */
  readOnly?: boolean;
  agentId: string;
  agentName: string;
  /** The agent is paused: a one-time schedule that is due reads "Waiting: <Agent> is paused" (P5-SCHED-ONCE). */
  agentFrozen?: boolean;
  /** `/o/<org>/w/<ws>/agents/<handle>`: the page a run's thread link starts from. */
  agentBase: string;
  orgId?: string;
  wsId?: string;
  canOwn: boolean;
  defaultTimezone?: string;
  workspaceName?: string;
  narrationCards?: boolean;
  initialFilter?: AutomationFilter;
}) {
  const schedules = useSchedules(agentId);
  const webhooks = useWebhooks(agentId);
  const presences = usePresences(agentId, !readOnly);
  const options = usePresenceOptions(agentId, !readOnly);
  const connectors = useConnectors();
  const who = useWhoCan({ workspaceId: wsId }, "workspace_owner", "add_schedule");
  // Who can connect a chat platform: the organization's owners and admins. Until the role is known, the workspace right stands in.
  const orgRole = useOrg(orgId).data?.my_role;
  const canConnect = orgRole ? orgRole === "owner" || orgRole === "admin" : canOwn;
  const [filter, setFilter] = useState<AutomationFilter>(readOnly && initialFilter === "chats" ? "all" : initialFilter);
  const [choosing, setChoosing] = useState(false);
  const [creating, setCreating] = useState<"schedule" | "webhook" | null>(null);
  const [reveal, setReveal] = useState<WebhookReveal | null>(null);
  const chatsRef = useRef<HTMLDivElement>(null);
  // (P5-REFINE-19) The palette's "New schedule for <Agent>" and "New webhook for <Agent>" open the form the New automation choice opens.
  useVerb("new-schedule", () => setCreating("schedule"), canOwn && !readOnly);
  useVerb("new-webhook", () => setCreating("webhook"), canOwn && !readOnly);
  // (P5-REFINE-19, `bulk`, Expert) A checkbox per schedule and webhook for an owner (a chat row has none), a bar above the list.
  const bulkOn = useBulkOn();
  // The selection is of what the filter shows: changing the filter drops the ticks that are no longer in the list.
  const selectable = canOwn
    ? [
        ...(filter === "all" || filter === "schedules" ? (schedules.data?.items ?? []).map((s) => s.id) : []),
        ...(filter === "all" || filter === "webhooks" ? (webhooks.data?.items ?? []).map((w) => w.id) : []),
      ]
    : [];
  const sel = useSelection(selectable);

  if (schedules.isPending || webhooks.isPending) return <Loading />;
  if (schedules.isError) return <ErrorState error={schedules.error} onRetry={() => void schedules.refetch()} />;
  if (webhooks.isError) return <ErrorState error={webhooks.error} onRetry={() => void webhooks.refetch()} />;

  const sch = schedules.data.items;
  const hooks = webhooks.data.items;
  const chats = presences.data?.items ?? [];
  const platformOf = (p: Presence) => connectors.data?.items.find((c) => c.id === p.connector_id)?.display_name ?? "a chat platform";
  const hasPlatform = (options.data?.items.length ?? 0) > 0 || chats.length > 0;
  const total = sch.length + hooks.length + chats.length;
  const show = (k: Exclude<AutomationFilter, "all">) => filter === "all" || filter === k;
  const picked: BulkAutomation[] = [
    ...sch.filter((s) => sel.selected.has(s.id)).map((s) => ({ id: s.id, name: s.name, kind: "schedule" as const, enabled: s.enabled, done: !s.cron && !!s.completed_at })),
    ...hooks.filter((w) => sel.selected.has(w.id)).map((w) => ({ id: w.id, name: w.name, kind: "webhook" as const, enabled: w.enabled })),
  ];
  const pick = (id: string) => (canOwn ? { checked: sel.selected.has(id), onChange: (on: boolean) => sel.toggle(id, on) } : undefined);
  const anyRow =(show("schedules") && sch.length > 0) || (show("webhooks") && hooks.length > 0) || (show("chats") && chats.length > 0);

  const start = (k: NewKind) => {
    setChoosing(false);
    if (k === "chat") {
      setFilter("chats");
      setTimeout(() => chatsRef.current?.scrollIntoView({ block: "start" }), 0);
    } else {
      setCreating(k);
    }
  };
  const newButton = readOnly ? null : (
    <Button size="sm" onClick={() => setChoosing(true)}>
      <PlusIcon aria-hidden="true" className="size-4" />
      New automation
    </Button>
  );
  const filters: { value: AutomationFilter; label: string }[] = [
    { value: "all", label: `All ${total}` },
    { value: "schedules", label: `Schedules ${sch.length}` },
    { value: "webhooks", label: `Webhooks ${hooks.length}` },
    ...(readOnly ? [] : [{ value: "chats" as const, label: `Chats ${chats.length}` }]),
  ];

  return (
    <Section
      id="automations"
      title="Automations"
      description={`When ${agentName} runs: on a schedule, when something calls its address, or in a chat.`}
      actions={newButton}
    >
      {creating === "schedule" ? (
        <ScheduleForm agentId={agentId} defaultTimezone={defaultTimezone} workspaceName={workspaceName} canOwn={canOwn} onDone={() => setCreating(null)} />
      ) : null}
      {creating === "webhook" ? <WebhookForm agentId={agentId} agentName={agentName} canOwn={canOwn} onDone={() => setCreating(null)} onReveal={setReveal} /> : null}

      {total === 0 && !creating ? (
        <EmptyState title="No automations yet" when={readOnly ? undefined : `${agentName} can run on a schedule, when something calls it, or in a chat.`} action={newButton} />
      ) : null}

      {total > 0 ? (
        <>
          <Segmented<AutomationFilter> label="Show" className="flex-wrap" value={filter} onChange={setFilter} options={filters} />
          {selectable.length > 0 ? (
            <>
              {bulkOn ? (
                <label className="flex items-center text-sm text-ink-2">
                  <BulkSelectAll label="Select all automations" all={sel.all} some={sel.some} onChange={sel.setAll} />
                  Select all
                </label>
              ) : null}
              <AutomationsBulkBar agentId={agentId} picked={picked} onClear={sel.clear} />
            </>
          ) : null}
          {anyRow ? (
            <ItemList aria-label="Automations">
              {show("schedules")
                ? sch.map((s) => (
                    <ScheduleRow key={s.id} agentId={agentId} s={s} agentBase={agentBase} agentName={agentName} agentFrozen={agentFrozen} defaultTimezone={defaultTimezone} workspaceName={workspaceName} wsId={wsId} canOwn={canOwn} select={pick(s.id)} readOnly={readOnly} />
                  ))
                : null}
              {show("webhooks")
                ? hooks.map((w) => <WebhookRow key={w.id} agentId={agentId} agentName={agentName} w={w} agentBase={agentBase} wsId={wsId} canOwn={canOwn} onReveal={setReveal} select={pick(w.id)} readOnly={readOnly} />)
                : null}
              {show("chats") ? chats.map((p) => <ChatRow key={p.id} agentId={agentId} agentName={agentName} p={p} platform={platformOf(p)} />) : null}
            </ItemList>
          ) : (
            <p className="text-sm text-muted">Nothing of this kind yet.</p>
          )}
        </>
      ) : null}

      {/* The Chats set-up: with a chat platform (or a chat the agent is in) it is the presences, bindings and reach; without, the ask. */}
      {/* Only an organization owner or admin can connect a platform, so with none the Chats group is where everyone else (a personal-workspace owner included) asks for one (channels-3). */}
      {!readOnly && show("chats") && (hasPlatform || filter === "chats" || !canConnect) ? (
        <div id="chats" ref={chatsRef} className="scroll-mt-4 space-y-3 pt-2">
          <h3 className="font-display text-base font-semibold text-ink">Chats</h3>
          {hasPlatform ? (
            <Altitude id="chat.setup" level="setup">
              <ChannelsBody agentId={agentId} orgId={orgId} canOwn={canOwn} wsId={wsId} narrationCards={narrationCards} />
            </Altitude>
          ) : (
            <NoChatYet agentName={agentName} wsId={wsId} />
          )}
        </div>
      ) : null}

      <Dialog open={choosing} onOpenChange={setChoosing}>
        <DialogContent title="New automation" description={`How should ${agentName} be started?`}>
          <div className="space-y-2">
            <KindChoice
              icon={<ClockIcon />}
              title="On a schedule"
              text={`${agentName} runs at set times.`}
              disabled={!canOwn}
              note={!canOwn ? <WhoCan value={{ ...who, action: "add_schedule" }} lead /> : undefined}
              onPick={() => start("schedule")}
            />
            <KindChoice icon={<LinkIcon />} title="When something calls it" text={`An outside system sends ${agentName} a request.`} onPick={() => start("webhook")} />
            {hasPlatform ? (
              <KindChoice
                icon={<MessageIcon />}
                title="In a chat"
                text={`${agentName} joins a channel on a chat platform.`}
                onPick={() => start("chat")}
              />
            ) : (
              // (P5-REFINE-14, channels-3) No platform: the same ask as the Chats set-up, with its optional reason.
              <NoChatYet agentName={agentName} wsId={wsId} />
            )}
          </div>
        </DialogContent>
      </Dialog>

      <WebhookRevealDialog reveal={reveal} onClose={() => setReveal(null)} />
    </Section>
  );
}

function KindChoice({
  icon,
  title,
  text,
  disabled,
  note,
  onPick,
}: {
  icon: React.ReactNode;
  title: string;
  text: string;
  disabled?: boolean;
  note?: React.ReactNode;
  onPick: () => void;
}) {
  return (
    <div>
      <button
        type="button"
        disabled={disabled}
        onClick={onPick}
        className="flex min-h-11 w-full items-start gap-3 rounded-control border border-line bg-surface p-3 text-left hover:border-line-strong disabled:opacity-60 dark:bg-canvas">
        <span aria-hidden="true" className="mt-0.5 flex size-8 shrink-0 items-center justify-center rounded-control bg-subtle text-ink-2 [&_svg]:size-4">
          {icon}
        </span>
        <span className="min-w-0">
          <span className="block text-sm font-semibold text-ink">{title}</span>
          <span className="block text-sm text-ink-2">{text}</span>
        </span>
      </button>
      {note ? <div className="mt-1 text-sm">{note}</div> : null}
    </div>
  );
}
