"use client";

import { useEffect, useMemo, useState, type ReactNode } from "react";

import { GrantEditor } from "@/components/agents/grant-editor";
import { GrantForm } from "@/components/grants/grant-form";
import { ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import { Notice } from "@/components/ui/notice";
import { ApiError } from "@/lib/api/client";
import { appRestrictions, useApp, useConnections } from "@/lib/api/apps";
import { appDisplayName } from "@/lib/api/app-name";
import { useGrants } from "@/lib/api/policy";
import { preselectAgents } from "@/lib/accounts";
import {
  grantValueBody,
  grantValueProblem,
  initialGrantValue,
  splitByScopes,
  toolsForMethod,
  type GrantValue,
} from "@/lib/grant-editor";
import {
  failingAgentId,
  useAddCredentialGrants,
  useCredentialGrants,
  useRemoveCredentialGrant,
  type GrantedAgent,
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
  /** Connections only: the app, to load its tools for the grant form (FIX-147). */
  appId?: string;
  /** Connections only: workspace used to read the app's tool list. */
  appWorkspaceId?: string;
  /** Connections only: how the connection was made; tools restricted to other methods are not offered (FIX-123). */
  connectMethod?: string | null;
  /** Connections only: the scopes the connection has; tools needing others are listed as unavailable (FIX-136). */
  scopes?: string[];
  /** Connections only: the app allows only custom grants (`custom_only`, P4-GRANTS), so the form starts on Custom. */
  customOnly?: boolean;
  /** Agents ticked when the picker opens (the getting-started guide preselects the workspace's own agent). */
  preselect?: string[];
  /**
   * (C-925) The agent whose page opened the picker: preselected. Without one, the only agent the person may choose
   * (a workspace's list) is preselected, else none.
   */
  contextAgentId?: string;
}

type AgentLite = { id: string; name: string; handle: string; kind: string };

function AgentChecklist({
  agents,
  has,
  selected,
  toggle,
  disabled,
  idp,
  onEdit,
  prefix,
}: {
  agents: AgentLite[];
  has: Set<string>;
  selected: Set<string>;
  toggle: (id: string) => void;
  disabled: boolean;
  idp: string;
  /** Connections: opens the agent's grant editor (the bulk call never changes an existing grant, C-504). */
  onEdit?: (agentId: string) => void;
  /** (C-948) The workspace's name, when agents of more than one workspace are listed: "<workspace> · <agent>". */
  prefix?: string;
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
              <span className="min-w-0 break-words font-medium text-ink">{prefix ? `${prefix} · ${a.name}` : a.name}</span>
              <span className="font-mono text-xs text-muted">@{a.handle}</span>
              {already ? <Badge tone="accent">Has access</Badge> : null}
              {already && onEdit ? (
                <Button type="button" size="sm" variant="outline" aria-label={`Edit access for ${a.name}`} onClick={() => onEdit(a.id)}>
                  Edit
                </Button>
              ) : null}
            </label>
          </li>
        );
      })}
    </ul>
  );
}

export function WorkspaceAgents({
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
  onEdit?: (agentId: string) => void;
  prefix?: string;
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
  onEdit?: (agentId: string) => void;
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
        <WorkspaceAgents key={w.id} wsId={w.id} name={w.name} prefix={owned.length > 1 ? w.name : undefined} register={register} {...rest} />
      ))}
    </div>
  );
}

/** Opens one agent's own grant editor (the same dialog as the Access tab) for an agent that already has the connection. */
function ExistingGrantEditor({ subject, agent, onClose }: { subject: GrantSubject; agent: GrantedAgent; onClose: () => void }) {
  const conns = useConnections(subject.scope);
  const grants = useGrants(agent.agent_id);
  if (conns.isPending || grants.isPending) return <Loading />;
  if (conns.isError) return <ErrorState error={conns.error} onRetry={() => conns.refetch()} />;
  if (grants.isError) return <ErrorState error={grants.error} onRetry={() => grants.refetch()} />;
  const connection = conns.data.items.find((c) => c.id === subject.target.id);
  if (!connection) return <ErrorState error={{ code: "not_found" }} />;
  const grant = grants.data.connections.find((g) => g.connection_id === connection.id) ?? null;
  return (
    <GrantEditor
      agentId={agent.agent_id}
      wsId={agent.workspace_id}
      connection={connection}
      grant={grant}
      restrictions={grant}
      open
      onOpenChange={(o) => {
        if (!o) onClose();
      }}
    />
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
  // (C-925) The contextual agent, else the only agent the person may choose, else none.
  // `picked` is what the person ticked; until they tick something the default below applies.
  const [picked, setPicked] = useState<Set<string> | null>(null);
  const [names, setNames] = useState<Record<string, string>>({});
  const [result, setResult] = useState<{ created: number; existing: number } | null>(null);
  // `full_grant_forbidden` (422) on a `custom_only` app: the form switches to Custom.
  const [forcedCustom, setForcedCustom] = useState(false);
  const isConnection = subject.target.kind === "connection";
  // Connections: the same grant form as the agent's Access tab (FIX-147), Full preselected unless the app is custom-only.
  const app = useApp(isConnection ? subject.appId : undefined, subject.appWorkspaceId);
  const split = useMemo(
    () => splitByScopes(toolsForMethod(app.data?.tools ?? [], subject.connectMethod), subject.scopes),
    [app.data, subject.connectMethod, subject.scopes],
  );
  const tools = split.usable;
  const restrictions = appRestrictions(app.data);
  const customOnly = isConnection && (subject.customOnly === true || forcedCustom || restrictions.custom_only);
  const [edited, setEdited] = useState<GrantValue | null>(null);
  const grantValue = edited ?? initialGrantValue(tools, null, customOnly);
  const [editing, setEditing] = useState<GrantedAgent | null>(null);

  const has = useMemo(() => new Set((grants.data ?? []).map((g) => g.agent_id)), [grants.data]);
  const register = useMemo(
    () => (agents: AgentLite[]) =>
      setNames((cur) => {
        if (agents.every((a) => cur[a.id] === a.name)) return cur;
        return { ...cur, ...Object.fromEntries(agents.map((a) => [a.id, a.name])) };
      }),
    [],
  );
  // The default (C-925): the guide's or the page's agent; else, in a workspace's list, the only agent the person may
  // choose (an organization's list loads per workspace, so it never guesses); else none.
  const explicit = subject.preselect ?? (subject.contextAgentId ? [subject.contextAgentId] : null);
  const defaults: string[] =
    explicit ??
    (subject.scope.kind === "workspace" && grants.data
      ? preselectAgents(
          Object.keys(names)
            .filter((id) => !has.has(id))
            .map((id) => ({ id })),
          null,
        )
      : []);
  const selected = picked ?? new Set(defaults);
  const setSelected = setPicked;
  const toggle = (id: string) => {
    const next = new Set(selected);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    setPicked(next);
  };
  const only = selected.size === 1 ? names[[...selected][0]] : undefined;

  const failing = add.error ? failingAgentId(add.error) : null;
  const errorText = add.error
    ? failing
      ? `Could not give access to ${names[failing] ?? "an agent"}: ${errorMessage(add.error)} No agent was changed.`
      : `${errorMessage(add.error)} No agent was changed.`
    : null;
  // The form needs the app's tools; without them (no app to read, still loading, failed) a custom-only app cannot be
  // chosen for, and a normal app waits rather than sending a choice nobody saw.
  const formReady = isConnection && !!subject.appId && !!app.data;
  const needsTools = isConnection && !!subject.appId ? !formReady || grantValueProblem(grantValue, customOnly) !== null : customOnly;

  function submit() {
    add.reset();
    setResult(null);
    add.mutate(
      { agentIds: [...selected], grant: formReady ? grantValueBody(grantValue, customOnly) : undefined },
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
      <AgentPicker
        subject={subject}
        register={register}
        has={has}
        selected={selected}
        toggle={toggle}
        disabled={add.isPending}
        idp={`ga-${subject.target.id}`}
        onEdit={isConnection ? (id) => setEditing((grants.data ?? []).find((g) => g.agent_id === id) ?? null) : undefined}
      />
      {editing ? <ExistingGrantEditor subject={subject} agent={editing} onClose={() => setEditing(null)} /> : null}
      {isConnection && subject.appId ? (
        app.isPending ? (
          <Loading />
        ) : app.isError ? (
          <ErrorState error={app.error} onRetry={() => app.refetch()} />
        ) : (
          <GrantForm
            appName={appDisplayName(app.data)}
            tools={tools}
            unavailable={split.unavailable}
            customOnly={customOnly}
            blockUntooled={restrictions.block_untooled}
            value={grantValue}
            onChange={setEdited}
            disabled={add.isPending}
            idPrefix="ga"
          />
        )
      ) : customOnly ? (
        <Notice tone="pending" role="status">
          This app only allows custom access. Choose the allowed tools on each agent&apos;s Apps &amp; access tab.
        </Notice>
      ) : null}
      {errorText ? (
        <Notice tone="danger" role="alert">
          {errorText}
        </Notice>
      ) : null}
      <div className="flex flex-wrap gap-2">
        <Button size="sm" disabled={add.isPending || selected.size === 0 || needsTools} onClick={submit}>
          {add.isPending ? "Giving access…" : only ? `Give ${only} access` : "Give access"}
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
      <DialogContent title={`Access to ${title}`} description="The agents that can use this credential." size="md">
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
