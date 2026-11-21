"use client";

import Link from "next/link";
import { useState, type FormEvent } from "react";

import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import { ChoiceCards } from "@/components/ui/choice-cards";
import { ConfirmDialog, FormDialog } from "@/components/ui/confirm-dialog";
import { Field } from "@/components/ui/field";
import { PlugIcon } from "@/components/ui/icons";
import { Menu, MenuContent, MenuItem, MenuTrigger } from "@/components/ui/menu";
import { ErrorNotice } from "@/components/ui/notice";
import { ItemList, ListRow, Section } from "@/components/ui/section";
import { Select } from "@/components/ui/select";
import { ApiError } from "@/lib/api/client";
import { errorMessage } from "@/lib/api/errors";
import { useCanApprove } from "@/lib/api/approvals";
import {
  LINK_DIRECTION_CHOICES,
  LINK_POLICIES,
  directionSentence,
  directionTitle,
  useAgentLinks,
  useCreateLink,
  useDeleteLink,
  OTHER_AGENT,
  useLinkableAgents,
  usePeerTasks,
  useSetLinkDirection,
  useSetLinkPolicy,
  type AgentLink,
  type LinkDirectionChoice,
  type LinkPolicy,
  type LinkableAgent,
} from "@/lib/api/peers";
import { formatExact } from "@/lib/format-date";
import { LINK_POLICY, PEER_TASK_STATUS, label } from "@/lib/labels";
import { personName } from "@/lib/workspace-name";

/** A failed link call in words (C-1085): the API's own message is for the log, not for the person. */
function linkErrorMessage(error: unknown): string {
  if (error instanceof ApiError) {
    switch (error.code) {
      case "stale_revision":
        return "Someone changed this link. The latest version is loaded; try again.";
      case "approver_required":
        return "Only an approver of both agents' workspaces can widen who may hand tasks to whom.";
      case "link_exists":
        return "These agents are already linked.";
      case "link_self":
        return "An agent can't be linked to itself.";
      case "not_hosted":
        return "That agent runs on its own computer, so it can't be linked.";
      case "frozen":
        return "One of these agents is paused, so it can't be linked.";
      case "forbidden":
      case "permission_denied":
        return "You aren't allowed to change this link.";
      case "not_found":
        return "That agent or link isn't there any more.";
      default:
    }
  }
  return errorMessage(error);
}

/** The failure as the dialog shows it: an error value whose message is already in words. */
function linkFailure(error: unknown): ApiError {
  return new ApiError("link_error", linkErrorMessage(error));
}

/** Why a candidate of the picker cannot be chosen, or null when it can. */
function whyNotLinkable(a: LinkableAgent): string | null {
  if (a.linked) return "Already linked";
  if (a.frozen) return "Paused";
  if (!a.hosted) return "Runs on its own computer";
  return null;
}

function DirectionCards({
  agentName,
  peerName,
  value,
  onChange,
  legend = "Who may hand tasks to whom?",
}: {
  agentName: string;
  peerName: string;
  value: LinkDirectionChoice;
  onChange: (v: LinkDirectionChoice) => void;
  legend?: string;
}) {
  return (
    <ChoiceCards<LinkDirectionChoice>
      legend={legend}
      value={value}
      onChange={onChange}
      options={LINK_DIRECTION_CHOICES.map((d) => ({
        value: d,
        label: directionTitle(agentName, peerName, d),
        description: directionSentence(agentName, peerName, d),
      }))}
    />
  );
}

/** The mockup's "Link another agent": a picker of `GET /agents/{agent}/linkable` and the three direction cards. */
function LinkDialog({ agentId, agentName, agentsHref, open, onOpenChange }: { agentId: string; agentName: string; agentsHref: string; open: boolean; onOpenChange: (o: boolean) => void }) {
  const linkable = useLinkableAgents(agentId, open);
  const create = useCreateLink(agentId);
  const [peerId, setPeerId] = useState("");
  const [direction, setDirection] = useState<LinkDirectionChoice>("both");
  const items = linkable.data?.items ?? [];
  const peer = items.find((a) => a.agent_id === peerId);
  const submit = (e: FormEvent<HTMLFormElement>) => {
    e.preventDefault();
    if (!peer) return;
    create.mutate(
      { peerAgentId: peer.agent_id, direction },
      {
        onSuccess: () => {
          onOpenChange(false);
        },
      },
    );
  };
  const change = (o: boolean) => {
    if (!o) {
      setPeerId("");
      setDirection("both");
      create.reset();
    }
    onOpenChange(o);
  };
  return (
    <FormDialog
      open={open}
      onOpenChange={change}
      title="Link another agent"
      description={`Linked agents can hand each other tasks. Choose who may start them.`}
      submitLabel="Link agents"
      busy={create.isPending}
      submitDisabled={!peer}
      onSubmit={submit}
      error={create.error ? linkFailure(create.error) : undefined}
    >
      {linkable.isPending ? <Loading /> : null}
      {linkable.isError ? <ErrorNotice error={linkable.error} /> : null}
      {linkable.data && items.length === 0 ? (
        <p className="text-sm text-ink-2">
          There is no other agent you can link. <Link className="underline" href={agentsHref}>Create one first.</Link>
        </p>
      ) : null}
      {linkable.data && items.length > 0 ? (
        <Field id="link-peer" label="Agent">
          <Select id="link-peer" value={peerId} onChange={setPeerId} placeholder="Choose an agent">
            {items.map((a) => {
              const why = whyNotLinkable(a);
              return (
                <option key={a.agent_id} value={a.agent_id} disabled={why !== null}>
                  {a.name} · {a.workspace_name}
                  {why ? ` (${why})` : ""}
                </option>
              );
            })}
          </Select>
        </Field>
      ) : null}
      {linkable.data && items.length > 0 ? (
        <DirectionCards agentName={agentName} peerName={peer?.name ?? OTHER_AGENT} value={direction} onChange={setDirection} />
      ) : null}
    </FormDialog>
  );
}

function LinkRow({ agentId, agentName, link, canApprove }: { agentId: string; agentName: string; link: AgentLink; canApprove: boolean }) {
  const setPolicy = useSetLinkPolicy(agentId);
  const setDirection = useSetLinkDirection(agentId);
  const remove = useDeleteLink(agentId);
  const [changing, setChanging] = useState(false);
  const [removing, setRemoving] = useState(false);
  const [choice, setChoice] = useState<LinkDirectionChoice>("both");
  const peer = link.peer.name;
  const error = setPolicy.error ?? setDirection.error ?? remove.error;
  const current: LinkDirectionChoice = link.direction === "outgoing" ? "to_peer" : link.direction === "incoming" ? "from_peer" : "both";
  const peerPolicy: Record<LinkPolicy, string> = {
    ask: `${peer} asks you first`,
    allow: `${peer} allows it`,
    blocked: `${peer} blocks it`,
  };
  const by =
    link.created_by.kind === "person"
      ? `Linked by ${link.created_by.display_name ? personName(link.created_by.display_name) : "someone who has left"}`
      : `Opened by ${agentName}`;
  return (
    <li className="space-y-2 p-3 sm:px-4">
      <div className="flex flex-wrap items-start gap-x-3 gap-y-1">
        <div className="min-w-0 flex-1">
          <p className="text-sm">
            <span className="font-medium text-ink">{peer}</span> <span className="text-muted">· {link.peer.workspace_name}</span>
          </p>
          <p className="text-sm text-ink-2">{directionSentence(agentName, peer, link.direction)}</p>
          <p className="text-xs text-muted">
            {by} · {peerPolicy[link.peer.policy]}
          </p>
        </div>
        <Menu>
          <MenuTrigger asChild>
            <Button variant="outline" size="sm" aria-label={`Actions for the link with ${peer}`}>
              Manage
            </Button>
          </MenuTrigger>
          <MenuContent align="end">
            <MenuItem
              onSelect={() => {
                setChoice(current);
                setDirection.reset();
                setChanging(true);
              }}
            >
              Change direction…
            </MenuItem>
            <MenuItem onSelect={() => setRemoving(true)}>Remove link…</MenuItem>
          </MenuContent>
        </Menu>
      </div>
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-sm">
        <label htmlFor={`pol-${link.id}`} className="font-medium text-ink">
          My policy for {peer}
        </label>
        <Select
          id={`pol-${link.id}`}
          className="max-w-48"
          value={link.self.policy}
          disabled={setPolicy.isPending}
          onChange={(v) => setPolicy.mutate({ link, policy: v as LinkPolicy })}
        >
          {LINK_POLICIES.map((p) => {
            const blocked = p === "allow" && !canApprove && link.self.policy !== "allow";
            return (
              <option key={p} value={p} disabled={blocked} title={blocked ? "Only an approver can allow this" : undefined}>
                {label(LINK_POLICY, p)}
                {blocked ? " (only an approver can allow this)" : ""}
              </option>
            );
          })}
        </Select>
      </div>
      {error ? (
        <p role="alert" className="rounded-control bg-danger-soft px-3 py-2 text-sm text-danger-ink">
          {linkErrorMessage(error)}
        </p>
      ) : null}
      <FormDialog
        open={changing}
        onOpenChange={setChanging}
        title={`Change direction with ${peer}`}
        description="Tasks already open or waiting are not affected."
        submitLabel="Save direction"
        busy={setDirection.isPending}
        submitDisabled={choice === current}
        onSubmit={(e) => {
          e.preventDefault();
          setDirection.mutate({ link, direction: choice }, { onSuccess: () => setChanging(false) });
        }}
        error={setDirection.error ? linkFailure(setDirection.error) : undefined}
      >
        <DirectionCards agentName={agentName} peerName={peer} value={choice} onChange={setChoice} legend="Who may hand tasks to whom?" />
      </FormDialog>
      <ConfirmDialog
        open={removing}
        onOpenChange={setRemoving}
        title={`Remove the link with ${peer}?`}
        description={`${agentName} and ${peer} will stop handing each other tasks. Tasks in progress between them are cancelled.`}
        confirmLabel="Remove link"
        danger
        busy={remove.isPending}
        onConfirm={() => remove.mutate(link, { onSuccess: () => setRemoving(false), onError: () => setRemoving(false) })}
      />
    </li>
  );
}

function Tasks({ agentId, agentName, peers }: { agentId: string; agentName: string; peers: Map<string, string> }) {
  const q = usePeerTasks(agentId);
  if (q.isPending) return <Loading />;
  if (q.isError) return <ErrorState error={q.error} onRetry={() => void q.refetch()} />;
  if (q.data.items.length === 0) return <p className="text-sm text-ink-2">No tasks from other agents yet.</p>;
  return (
    <ItemList aria-label="Tasks between agents">
      {q.data.items.map((t) => {
        const other = peers.get(t.direction === "outgoing" ? t.to_agent_id : t.from_agent_id) ?? "another agent";
        const sentence =
          t.direction === "outgoing" ? `${agentName} handed a task to ${other}` : `${other} handed a task to ${agentName}`;
        return (
          <ListRow key={t.id}>
            <div className="min-w-0 flex-1 space-y-1 text-sm">
              <p className="text-ink">
                {sentence} · {label(PEER_TASK_STATUS, t.status)} · {formatExact(t.created_at)}
              </p>
              {t.requester ? <p className="text-xs text-muted">Asked for by {personName(t.requester.display_name)}</p> : null}
              {t.report ? <p className="whitespace-pre-wrap text-ink-2">{t.report}</p> : null}
            </div>
          </ListRow>
        );
      })}
    </ItemList>
  );
}

/** Settings › Other agents (P5-REFINE-12, channels-1, D-411): link, re-direct and remove links; the tasks below. */
export function PeersTab({ agentId, agentName, agentsHref, wsId, frozen = false }: { agentId: string; agentName: string; agentsHref: string; wsId?: string; frozen?: boolean }) {
  const links = useAgentLinks(agentId);
  const tasks = usePeerTasks(agentId);
  const canApprove = useCanApprove(wsId);
  const [linking, setLinking] = useState(false);
  const linkButton = (
    <Button type="button" onClick={() => setLinking(true)} disabled={frozen} title={frozen ? `${agentName} is paused` : undefined}>
      Link another agent
    </Button>
  );
  const dialog = <LinkDialog agentId={agentId} agentName={agentName} agentsHref={agentsHref} open={linking} onOpenChange={setLinking} />;
  if (links.isPending) return <Loading />;
  if (links.isError) return <ErrorState error={links.error} onRetry={() => void links.refetch()} />;
  if (links.data.items.length === 0 && tasks.data && tasks.data.items.length === 0) {
    return (
      <>
        <EmptyState title={`${agentName} isn't linked to another agent`} icon={<PlugIcon />} action={linkButton}>
          Linked agents can hand each other tasks. The tasks they exchange appear here.
        </EmptyState>
        {dialog}
      </>
    );
  }
  const peers = new Map(links.data.items.map((l) => [l.peer.agent_id, l.peer.name]));
  return (
    <div className="space-y-8">
      <Section id="agent-links" title="Linked agents" description={`Agents ${agentName} may hand tasks to or accept tasks from.`} actions={linkButton}>
        {links.data.items.length === 0 ? (
          <p className="text-sm text-ink-2">{agentName} isn&apos;t linked to another agent.</p>
        ) : (
          <ItemList aria-label="Linked agents">
            {links.data.items.map((l) => (
              <LinkRow key={l.id} agentId={agentId} agentName={agentName} link={l} canApprove={canApprove} />
            ))}
          </ItemList>
        )}
      </Section>
      <Section id="peer-tasks" title="Tasks between agents">
        <Tasks agentId={agentId} agentName={agentName} peers={peers} />
      </Section>
      {dialog}
    </div>
  );
}
