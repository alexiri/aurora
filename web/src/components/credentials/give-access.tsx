"use client";

import { useEffect, useMemo, useState, type ReactNode } from "react";

import { ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import { Notice } from "@/components/ui/notice";
import { ApiError } from "@/lib/api/client";
import { useApp } from "@/lib/api/apps";
import {
  failingAgentId,
  useAddCredentialGrants,
  useCredentialGrants,
  useRemoveCredentialGrant,
  type GrantTarget,
} from "@/lib/api/credential-grants";
import { errorMessage } from "@/lib/api/errors";
import { useAgents, useWorkspaces } from "@/lib/api/queries";
import type { SecretScope } from "@/lib/api/secrets";

/** What the picker needs to know about the credential being shared. */
export interface GrantSubject {
  target: GrantTarget;
  /** Where the credential lives: the agent list follows it (workspace: its agents; organization: the caller's workspaces). */
  scope: SecretScope;
  /** Needed for an organization scope (the workspaces list). */
  orgId: string;
  /** Connections only: the app, to load its tools when the grant must be custom. */
  appId?: string;
  /** Connections only: workspace used to read the app's tool list. */
  appWorkspaceId?: string;
  /** Connections only: the app allows only custom grants (`custom_only`, P4-GRANTS), so ask for tools first. */
  customOnly?: boolean;
}

type AgentLite = { id: string; name: string; handle: string; kind: string };

function AgentChecklist({
  agents,
  has,
  selected,
  toggle,
  disabled,
  idp,
}: {
  agents: AgentLite[];
  has: Set<string>;
  selected: Set<string>;
  toggle: (id: string) => void;
  disabled: boolean;
  idp: string;
}) {
  if (agents.length === 0) return <p className="text-sm text-muted">No agents.</p>;
  return (
    <ul className="space-y-1">
      {agents.map((a) => {
        const already = has.has(a.id);
        return (
          <li key={a.id}>
            <label className="flex min-h-9 items-center gap-2.5 text-sm">
              <input
                type="checkbox"
                className="size-4 shrink-0 accent-accent"
                id={`${idp}-${a.id}`}
                checked={already || selected.has(a.id)}
                disabled={already || disabled}
                onChange={() => toggle(a.id)}
                aria-label={`Give ${a.name} access`}
              />
              <span className="min-w-0 break-words font-medium text-ink">{a.name}</span>
              <span className="font-mono text-xs text-muted">@{a.handle}</span>
              {already ? <Badge tone="accent">Has access</Badge> : null}
            </label>
          </li>
        );
      })}
    </ul>
  );
}

function WorkspaceAgents({
  wsId,
  name,
  register,
  ...rest
}: {
  wsId: string;
  name?: string;
  register: (agents: AgentLite[]) => void;
  has: Set<string>;
  selected: Set<string>;
  toggle: (id: string) => void;
  disabled: boolean;
  idp: string;
}) {
  const agents = useAgents(wsId);
  const items = useMemo(() => agents.data?.items ?? [], [agents.data]);
  useEffect(() => register(items), [items, register]);
  const body = agents.isPending ? (
    <Loading />
  ) : agents.isError ? (
    <ErrorState error={agents.error} onRetry={() => agents.refetch()} />
  ) : (
    <AgentChecklist agents={items} {...rest} />
  );
  if (!name) return body;
  return (
    <fieldset className="min-w-0 space-y-1">
      <legend className="mb-1 text-sm font-semibold text-ink">{name}</legend>
      {body}
    </fieldset>
  );
}

/** Workspace scope: the workspace's agents. Organization scope: grouped by the workspaces the caller owns. */
function AgentPicker({
  subject,
  register,
  ...rest
}: {
  subject: GrantSubject;
  register: (agents: AgentLite[]) => void;
  has: Set<string>;
  selected: Set<string>;
  toggle: (id: string) => void;
  disabled: boolean;
  idp: string;
}) {
  const orgScope = subject.scope.kind === "organization";
  const workspaces = useWorkspaces(orgScope ? subject.orgId : undefined);
  if (!orgScope) return <WorkspaceAgents wsId={subject.scope.id} register={register} {...rest} />;
  if (workspaces.isPending) return <Loading />;
  if (workspaces.isError) return <ErrorState error={workspaces.error} onRetry={() => workspaces.refetch()} />;
  const owned = workspaces.data.items.filter((w) => w.my_role === "owner");
  if (owned.length === 0) return <p className="text-sm text-muted">You do not own any workspace in this organization, so there are no agents to choose.</p>;
  return (
    <div className="space-y-3">
      {owned.map((w) => (
        <WorkspaceAgents key={w.id} wsId={w.id} name={w.name} register={register} {...rest} />
      ))}
    </div>
  );
}

/** A custom connection grant needs the allowed tools; every other tool is blocked. */
function ToolsChoice({
  subject,
  chosen,
  onChange,
}: {
  subject: GrantSubject;
  chosen: Set<string>;
  onChange: (next: Set<string>) => void;
}) {
  const app = useApp(subject.appId, subject.appWorkspaceId);
  if (!subject.appId || !subject.appWorkspaceId)
    return <Notice tone="pending" role="status">This app only allows custom access. Choose the allowed tools on each agent&apos;s Access tab.</Notice>;
  if (app.isPending) return <Loading />;
  if (app.isError) return <ErrorState error={app.error} onRetry={() => app.refetch()} />;
  const tools = app.data.tools;
  return (
    <fieldset className="min-w-0 space-y-1" aria-describedby="ga-tools-note">
      <legend className="mb-1 text-sm font-semibold text-ink">Allowed tools</legend>
      <p id="ga-tools-note" className="text-xs text-muted">
        This app only allows custom access. Every tool you do not choose is blocked for the agents you select.
      </p>
      {tools.map((t) => (
        <label key={t.id} className="flex min-h-9 items-center gap-2.5 text-sm">
          <input
            type="checkbox"
            className="size-4 shrink-0 accent-accent"
            checked={chosen.has(t.id)}
            onChange={(e) => {
              const next = new Set(chosen);
              if (e.target.checked) next.add(t.id);
              else next.delete(t.id);
              onChange(next);
            }}
          />
          <span className="min-w-0 break-words">{t.name}</span>
          <Badge className="normal-case tracking-normal">{t.group === "write" ? "Write" : "Read"}</Badge>
        </label>
      ))}
    </fieldset>
  );
}

/**
 * The "Give agents access" picker (D-137): the agents the caller can grant, nothing preselected, "Give access" and a
 * dismiss button ("Skip" in the step, "Close" in the dialog). Agents that already have the credential show as such.
 */
export function GiveAccessPanel({
  subject,
  dismissLabel = "Skip",
  onDismiss,
  heading = true,
}: {
  subject: GrantSubject;
  dismissLabel?: string;
  onDismiss: () => void;
  heading?: boolean;
}) {
  const grants = useCredentialGrants(subject.target);
  const add = useAddCredentialGrants(subject.target);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [names, setNames] = useState<Record<string, string>>({});
  const [result, setResult] = useState<{ created: number; existing: number } | null>(null);
  const [tools, setTools] = useState<Set<string>>(new Set());
  // `full_grant_forbidden` (422) on a `custom_only` app: ask for the tools and send a custom grant.
  const [forcedCustom, setForcedCustom] = useState(false);
  const customMode = subject.target.kind === "connection" && (subject.customOnly === true || forcedCustom);

  const has = useMemo(() => new Set((grants.data ?? []).map((g) => g.agent_id)), [grants.data]);
  const register = useMemo(
    () => (agents: AgentLite[]) =>
      setNames((cur) => {
        if (agents.every((a) => cur[a.id] === a.name)) return cur;
        return { ...cur, ...Object.fromEntries(agents.map((a) => [a.id, a.name])) };
      }),
    [],
  );
  const toggle = (id: string) =>
    setSelected((cur) => {
      const next = new Set(cur);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });

  const failing = add.error ? failingAgentId(add.error) : null;
  const errorText = add.error
    ? failing
      ? `Could not give access to ${names[failing] ?? "an agent"}: ${errorMessage(add.error)} No agent was changed.`
      : `${errorMessage(add.error)} No agent was changed.`
    : null;
  const needsTools = customMode && tools.size === 0;

  function submit() {
    add.reset();
    setResult(null);
    add.mutate(
      { agentIds: [...selected], custom: customMode ? { allow_tools: [...tools] } : undefined },
      {
        onSuccess: (r) => {
          setResult({ created: r.created.length, existing: r.existing.length });
          setSelected(new Set());
        },
        onError: (e) => {
          if (e instanceof ApiError && e.code === "full_grant_forbidden") setForcedCustom(true);
        },
      },
    );
  }

  return (
    <div className="space-y-3" aria-label="Give agents access">
      {heading ? <h3 className="font-display text-base font-semibold text-ink">Give agents access</h3> : null}
      {result ? (
        <Notice tone="accent" role="status">
          Access given to {result.created} {result.created === 1 ? "agent" : "agents"}
          {result.existing ? ` (${result.existing} already had it)` : ""}.
        </Notice>
      ) : null}
      {grants.isError ? <ErrorState error={grants.error} onRetry={() => grants.refetch()} /> : null}
      <AgentPicker subject={subject} register={register} has={has} selected={selected} toggle={toggle} disabled={add.isPending} idp={`ga-${subject.target.id}`} />
      {customMode ? <ToolsChoice subject={subject} chosen={tools} onChange={setTools} /> : null}
      {errorText ? (
        <Notice tone="danger" role="alert">
          {errorText}
        </Notice>
      ) : null}
      <div className="flex flex-wrap gap-2">
        <Button size="sm" disabled={add.isPending || selected.size === 0 || needsTools} onClick={submit}>
          {add.isPending ? "Giving access…" : "Give access"}
        </Button>
        <Button size="sm" variant="outline" onClick={onDismiss}>
          {result ? "Done" : dismissLabel}
        </Button>
      </div>
    </div>
  );
}

/** The step shown in place of a create form once the credential exists: a heading, what was saved, and the picker. */
export function GiveAccessStep({
  subject,
  savedLabel,
  onDone,
  children,
}: {
  subject: GrantSubject;
  savedLabel: string;
  onDone: () => void;
  /** Extra facts about what was saved (for an LLM key: the verified account). */
  children?: ReactNode;
}) {
  return (
    <section
      aria-label="Give agents access"
      className="space-y-3 rounded-card border border-line bg-surface p-4 text-ink shadow-card sm:p-5"
    >
      <p role="status" className="text-sm font-medium">
        Saved {savedLabel}
      </p>
      {children}
      <GiveAccessPanel subject={subject} onDismiss={onDone} />
    </section>
  );
}

/** The row "Access" dialog (owners): the agents that have the credential, "Remove" per agent, and the add picker. */
export function AccessDialog({
  subject,
  title,
  open,
  onOpenChange,
}: {
  subject: GrantSubject;
  title: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent title={`Access to ${title}`} description="The agents that can use this credential." className="w-[min(34rem,calc(100vw-2rem))]">
        {open ? <AccessDialogBody subject={subject} onClose={() => onOpenChange(false)} /> : null}
      </DialogContent>
    </Dialog>
  );
}

function AccessDialogBody({ subject, onClose }: { subject: GrantSubject; onClose: () => void }) {
  const grants = useCredentialGrants(subject.target);
  const remove = useRemoveCredentialGrant(subject.target);
  const [removing, setRemoving] = useState<string | null>(null);
  const [removeError, setRemoveError] = useState<{ name: string; error: unknown } | null>(null);
  const items = grants.data ?? [];
  const grouped = subject.scope.kind === "organization";

  return (
    <div className="space-y-5">
      <section aria-label="Agents with access" className="space-y-2">
        <h3 className="text-sm font-semibold text-ink">Agents with access</h3>
        {grants.isPending ? <Loading /> : null}
        {grants.isError ? <ErrorState error={grants.error} onRetry={() => grants.refetch()} /> : null}
        {grants.data && items.length === 0 ? <p className="text-sm text-muted">No agent has access yet.</p> : null}
        {items.length > 0 ? (
          <ul className="divide-y divide-line rounded-control border border-line">
            {items.map((g) => (
              <li key={g.agent_id} className="flex flex-wrap items-center justify-between gap-x-3 gap-y-1 px-3 py-2">
                <span className="min-w-0 text-sm">
                  <span className="break-words font-medium text-ink">{g.agent_name}</span>
                  {grouped ? <span className="text-muted"> · {g.workspace_name}</span> : null}
                </span>
                <Button
                  size="sm"
                  variant="danger"
                  aria-label={`Remove access for ${g.agent_name}`}
                  disabled={remove.isPending && removing === g.agent_id}
                  onClick={() => {
                    setRemoving(g.agent_id);
                    setRemoveError(null);
                    remove.mutate(g.agent_id, { onError: (error) => setRemoveError({ name: g.agent_name, error }) });
                  }}
                >
                  Remove
                </Button>
              </li>
            ))}
          </ul>
        ) : null}
        {removeError ? (
          <Notice tone="danger" role="alert">
            Could not remove access for {removeError.name}: {errorMessage(removeError.error)}
          </Notice>
        ) : null}
      </section>
      <section aria-label="Add agents" className="space-y-2">
        <h3 className="text-sm font-semibold text-ink">Add agents</h3>
        <GiveAccessPanel subject={subject} heading={false} dismissLabel="Close" onDismiss={onClose} />
      </section>
    </div>
  );
}
