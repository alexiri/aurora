"use client";

import { useState } from "react";

import { DynamicForm } from "@/components/apps/dynamic-form";
import { fmtDate } from "@/components/account/key-table";
import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import { inputClass } from "@/components/ui/field";
import { Notice } from "@/components/ui/notice";
import { ItemList, Section } from "@/components/ui/section";
import { defaultValues, validateFields, visibleFields, type FieldValues } from "@/lib/api/apps";
import {
  useAttachPresence,
  useDecideReach,
  useDeleteSpaceBinding,
  useDetachPresence,
  usePatchSpaceBinding,
  usePresenceOptions,
  usePresences,
  useReach,
  useReattachPresence,
  useSpaceBindings,
  type Listen,
  type Presence,
  type PresenceMode,
} from "@/lib/api/channels";
import { dedicatedFields, formFields, useConnectors, type ConnectorInfo } from "@/lib/api/connectors";

import { MutationError, StatusChip } from "./shared";

function AttachForm({ agentId }: { agentId: string }) {
  const options = usePresenceOptions(agentId);
  const attach = useAttachPresence(agentId);
  const [integrationId, setIntegrationId] = useState("");
  const [mode, setMode] = useState<PresenceMode | "">("");
  const [values, setValues] = useState<FieldValues>({});
  const [errors, setErrors] = useState<Record<string, string>>({});

  if (options.isPending) return <Loading />;
  if (options.isError) return <ErrorState error={options.error} onRetry={() => options.refetch()} />;
  const usable = options.data.items.filter((o) => !o.attached);
  if (usable.length === 0) {
    return <p className="text-sm text-muted">No active integration is left to attach to.</p>;
  }
  const integration = usable.find((o) => o.integration_id === integrationId);
  const modes = integration?.modes ?? [];
  const dedicated = integration ? formFields((integration.presence_setup as { dedicated?: unknown }).dedicated) : [];
  const fields = mode === "dedicated" ? dedicated : [];

  function submit(e: React.FormEvent) {
    e.preventDefault();
    if (!integration || !mode) return;
    const errs = validateFields(fields, values);
    setErrors(errs);
    if (Object.keys(errs).length) return;
    const sent: Record<string, string> = {};
    for (const f of visibleFields(fields, values)) if (values[f.name]) sent[f.name] = values[f.name];
    attach.mutate(
      { integration_id: integration.integration_id, mode, ...(fields.length ? { fields: sent } : {}) },
      {
        onSuccess: () => {
          setIntegrationId("");
          setMode("");
          setValues({});
        },
      },
    );
  }

  return (
    <form className="space-y-4 rounded-card border border-line bg-surface p-4 shadow-card sm:p-5" onSubmit={submit} aria-busy={attach.isPending}>
      <h3 className="font-display text-base font-semibold text-ink">Attach to an integration</h3>
      <div className="space-y-1.5">
        <label htmlFor="att-int" className="block text-sm font-medium text-ink">
          Integration
        </label>
        <select
          id="att-int"
          className={inputClass}
          value={integrationId}
          onChange={(e) => {
            setIntegrationId(e.target.value);
            setMode("");
            setValues({});
          }}
        >
          <option value="">Select…</option>
          {usable.map((i) => (
            <option key={i.integration_id} value={i.integration_id}>
              {i.connector_name} · {i.tenant_label}
            </option>
          ))}
        </select>
      </div>
      {integration ? (
        modes.length === 0 ? (
          <p className="text-sm text-muted">This integration offers no way to attach an agent.</p>
        ) : (
          <div className="space-y-1.5">
            <label htmlFor="att-mode" className="block text-sm font-medium text-ink">
              Identity
            </label>
            <select
              id="att-mode"
              className={inputClass}
              value={mode}
              onChange={(e) => {
                setMode(e.target.value as PresenceMode);
                setValues(defaultValues(dedicated));
              }}
            >
              <option value="">Select…</option>
              {modes.includes("dedicated") ? <option value="dedicated">Dedicated bot for this agent</option> : null}
              {modes.includes("shared") ? <option value="shared">Shared bot</option> : null}
            </select>
          </div>
        )
      ) : null}
      {fields.length ? (
        <DynamicForm
          fields={fields}
          values={values}
          errors={errors}
          onChange={(n, v) => setValues((p) => ({ ...p, [n]: v }))}
        />
      ) : null}
      <MutationError error={attach.error} />
      <Button type="submit" disabled={!integration || !mode || attach.isPending}>
        {attach.isPending ? "Attaching…" : "Attach"}
      </Button>
    </form>
  );
}

function PresenceRow({ agentId, p, connector }: { agentId: string; p: Presence; connector?: ConnectorInfo }) {
  const reattach = useReattachPresence(agentId);
  const detach = useDetachPresence(agentId);
  const fields = connector && p.mode === "dedicated" ? dedicatedFields(connector) : [];
  const [open, setOpen] = useState(false);
  const [values, setValues] = useState<FieldValues>({});
  const [errors, setErrors] = useState<Record<string, string>>({});
  return (
    <li className="space-y-3 p-3 sm:px-4">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
        <span className="font-medium text-ink">{connector?.display_name ?? p.connector_id}</span>
        <span className="text-sm text-ink-2">{p.tenant_label}</span>
        <span className="text-xs text-muted">{p.mode}</span>
        {p.handle ? <code className="font-mono text-xs text-ink-2">{p.handle}</code> : null}
        <StatusChip status={p.status} />
      </div>
      {p.status_reason ? (
        <Notice tone="pending" role="status">
          {p.status_reason}
        </Notice>
      ) : null}
      <div className="flex flex-wrap gap-2">
        <Button
          variant="outline"
          size="sm"
          disabled={reattach.isPending}
          onClick={() => (fields.length ? setOpen((v) => !v) : reattach.mutate({ id: p.id }))}
        >
          Re-attach
        </Button>
        <Button variant="outline" size="sm" disabled={detach.isPending} onClick={() => detach.mutate(p.id)}>
          Detach
        </Button>
      </div>
      {open && fields.length ? (
        <form
          className="space-y-4 rounded-card border border-line bg-subtle p-3 sm:p-4"
          onSubmit={(e) => {
            e.preventDefault();
            const errs = validateFields(fields, values);
            setErrors(errs);
            if (Object.keys(errs).length) return;
            const sent: Record<string, string> = {};
            for (const f of visibleFields(fields, values)) if (values[f.name]) sent[f.name] = values[f.name];
            reattach.mutate({ id: p.id, fields: sent }, { onSuccess: () => setOpen(false) });
          }}
        >
          <DynamicForm
            fields={fields}
            values={values}
            errors={errors}
            onChange={(n, v) => setValues((s) => ({ ...s, [n]: v }))}
          />
          <Button type="submit" size="sm" disabled={reattach.isPending}>
            Re-attach with these values
          </Button>
        </form>
      ) : null}
      <MutationError error={reattach.error ?? detach.error} />
    </li>
  );
}

function Presences({ agentId }: { agentId: string }) {
  const q = usePresences(agentId);
  const connectors = useConnectors();
  if (q.isPending) return <Loading />;
  if (q.isError) return <ErrorState error={q.error} onRetry={() => q.refetch()} />;
  const byId = new Map((connectors.data?.items ?? []).map((c) => [c.id, c]));
  return (
    <Section id="presences" title="Presences" description="Where this agent is present on chat platforms.">
      {q.data.items.length === 0 ? (
        <EmptyState title="Not attached anywhere">Attach this agent to a chat platform below.</EmptyState>
      ) : (
        <ItemList aria-label="Presence list">
          {q.data.items.map((p) => (
            <PresenceRow key={p.id} agentId={agentId} p={p} connector={byId.get(p.connector_id)} />
          ))}
        </ItemList>
      )}
      <AttachForm agentId={agentId} />
    </Section>
  );
}

const LISTEN: Listen[] = ["mentions", "all"];

function Bindings({ agentId }: { agentId: string }) {
  const q = useSpaceBindings(agentId);
  const patch = usePatchSpaceBinding(agentId);
  const del = useDeleteSpaceBinding(agentId);
  if (q.isPending) return <Loading />;
  if (q.isError) return <ErrorState error={q.error} onRetry={() => q.refetch()} />;
  return (
    <Section id="space-bindings" title="Space bindings" description="Chat spaces this agent listens in.">
      {q.data.items.length === 0 ? (
        <p className="text-sm text-muted">No spaces are bound to this agent.</p>
      ) : (
        <ItemList aria-label="Space binding list">
          {q.data.items.map((b) => (
            <li key={b.id} className="flex flex-wrap items-center gap-x-4 gap-y-2 p-3 text-sm sm:px-4">
              <span className="font-medium text-ink">{b.space_label}</span>
              <label className="flex items-center gap-2 text-ink-2">
                Listen
                <select
                  aria-label={`Listen mode for ${b.space_label}`}
                  className={inputClass + " w-auto"}
                  value={b.listen}
                  disabled={patch.isPending}
                  onChange={(e) => patch.mutate({ id: b.id, body: { listen: e.target.value as Listen } })}
                >
                  {LISTEN.map((l) => (
                    <option key={l} value={l}>
                      {l}
                    </option>
                  ))}
                </select>
              </label>
              <label className="flex items-center gap-2 text-ink-2">
                <input
                  type="checkbox"
                  className="size-4 shrink-0 accent-accent"
                  checked={b.followups_need_mention}
                  disabled={patch.isPending}
                  onChange={(e) => patch.mutate({ id: b.id, body: { followups_need_mention: e.target.checked } })}
                />
                Follow-ups need a mention
              </label>
              <Button
                variant="outline"
                size="sm"
                className="sm:ml-auto"
                aria-label={`Remove binding ${b.space_label}`}
                disabled={del.isPending}
                onClick={() => del.mutate(b.id)}
              >
                Remove
              </Button>
            </li>
          ))}
        </ItemList>
      )}
      <MutationError error={patch.error ?? del.error} />
    </Section>
  );
}

function Reach({ agentId, canOwn }: { agentId: string; canOwn: boolean }) {
  const q = useReach(agentId);
  const decide = useDecideReach(agentId);
  if (q.isPending) return <Loading />;
  if (q.isError) return <ErrorState error={q.error} onRetry={() => q.refetch()} />;
  const pending = q.data.items.filter((g) => g.state === "pending");
  const history = q.data.items.filter((g) => g.state !== "pending");
  return (
    <Section id="reach-requests" title="Reach requests" description="People who asked to reach this agent from a chat platform.">
      {pending.length === 0 ? (
        <p className="text-sm text-muted">No pending requests.</p>
      ) : (
        <ItemList aria-label="Pending reach requests">
          {pending.map((g) => (
            <li key={g.id} className="flex flex-wrap items-center gap-x-3 gap-y-2 p-3 text-sm sm:px-4">
              <span className="font-medium text-ink">{g.subject_label}</span>
              <span className="text-xs text-muted">{g.subject_kind}</span>
              <span className="text-xs text-muted">Requested {fmtDate(g.created_at)}</span>
              {canOwn ? (
                <span className="flex gap-2 sm:ml-auto">
                  <Button
                    variant="approve"
                    size="sm"
                    aria-label={`Approve ${g.subject_label}`}
                    disabled={decide.isPending}
                    onClick={() => decide.mutate({ id: g.id, body: { decision: "approve", listen: g.listen, revision: g.revision } })}
                  >
                    Approve
                  </Button>
                  <Button
                    variant="outline"
                    size="sm"
                    aria-label={`Block ${g.subject_label}`}
                    disabled={decide.isPending}
                    onClick={() => decide.mutate({ id: g.id, body: { decision: "block", revision: g.revision } })}
                  >
                    Block
                  </Button>
                </span>
              ) : null}
            </li>
          ))}
        </ItemList>
      )}
      <MutationError error={decide.error} />
      {history.length ? (
        <details className="space-y-2">
          <summary className="cursor-pointer text-sm font-medium text-ink">History ({history.length})</summary>
          <ItemList aria-label="Reach history" className="mt-2 text-sm">
            {history.map((g) => (
              <li key={g.id} className="flex flex-wrap items-center gap-x-3 gap-y-1 p-3 sm:px-4">
                <span className="text-ink">{g.subject_label}</span>
                <StatusChip status={g.state} />
                <span className="text-xs text-muted">
                  {g.decided_by ? `${g.decided_by.display_name} · ` : ""}
                  {fmtDate(g.decided_at ?? g.created_at)}
                </span>
              </li>
            ))}
          </ItemList>
        </details>
      ) : null}
    </Section>
  );
}

export function AgentChannelsTab({ agentId, canOwn }: { agentId: string; orgId?: string; canOwn: boolean }) {
  return (
    <div className="space-y-8">
      <Presences agentId={agentId} />
      {canOwn ? <Bindings agentId={agentId} /> : null}
      <Reach agentId={agentId} canOwn={canOwn} />
    </div>
  );
}
