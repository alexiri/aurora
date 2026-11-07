"use client";

import { useState } from "react";

import { DynamicForm } from "@/components/apps/dynamic-form";
import { fmtDate } from "@/components/account/key-table";
import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import { inputClass } from "@/components/ui/field";
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
    return <p className="text-sm text-neutral-500">No active integration is left to attach to.</p>;
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
    <form className="space-y-3 rounded-md border border-neutral-200 p-3" onSubmit={submit} aria-busy={attach.isPending}>
      <h3 className="font-medium">Attach to an integration</h3>
      <div>
        <label htmlFor="att-int" className="block text-sm font-medium">
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
          <p className="text-sm text-neutral-500">This integration offers no way to attach an agent.</p>
        ) : (
          <div>
            <label htmlFor="att-mode" className="block text-sm font-medium">
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
    <li className="space-y-2 rounded-md border border-neutral-200 p-3">
      <div className="flex flex-wrap items-center gap-2">
        <span className="font-medium">{connector?.display_name ?? p.connector_id}</span>
        <span className="text-sm text-neutral-600">{p.tenant_label}</span>
        <span className="text-xs text-neutral-500">{p.mode}</span>
        {p.handle ? <code className="text-xs">{p.handle}</code> : null}
        <StatusChip status={p.status} />
      </div>
      {p.status_reason ? <p className="text-sm text-amber-800">{p.status_reason}</p> : null}
      <div className="flex gap-2">
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
          className="space-y-2"
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
    <section aria-label="Presences" className="space-y-3">
      <h2 className="font-medium">Presences</h2>
      {q.data.items.length === 0 ? (
        <EmptyState title="Not attached anywhere">Attach this agent to a chat platform below.</EmptyState>
      ) : (
        <ul className="space-y-2">
          {q.data.items.map((p) => (
            <PresenceRow key={p.id} agentId={agentId} p={p} connector={byId.get(p.connector_id)} />
          ))}
        </ul>
      )}
      <AttachForm agentId={agentId} />
    </section>
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
    <section aria-label="Space bindings" className="space-y-2">
      <h2 className="font-medium">Space bindings</h2>
      {q.data.items.length === 0 ? (
        <p className="text-sm text-neutral-500">No spaces are bound to this agent.</p>
      ) : (
        <ul className="divide-y divide-neutral-200">
          {q.data.items.map((b) => (
            <li key={b.id} className="flex flex-wrap items-center gap-3 py-2 text-sm">
              <span className="font-medium">{b.space_label}</span>
              <label className="flex items-center gap-1">
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
              <label className="flex items-center gap-1">
                <input
                  type="checkbox"
                  checked={b.followups_need_mention}
                  disabled={patch.isPending}
                  onChange={(e) => patch.mutate({ id: b.id, body: { followups_need_mention: e.target.checked } })}
                />
                Follow-ups need a mention
              </label>
              <Button
                variant="outline"
                size="sm"
                className="ml-auto"
                aria-label={`Remove binding ${b.space_label}`}
                disabled={del.isPending}
                onClick={() => del.mutate(b.id)}
              >
                Remove
              </Button>
            </li>
          ))}
        </ul>
      )}
      <MutationError error={patch.error ?? del.error} />
    </section>
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
    <section aria-label="Reach requests" className="space-y-2">
      <h2 className="font-medium">Reach requests</h2>
      {pending.length === 0 ? (
        <p className="text-sm text-neutral-500">No pending requests.</p>
      ) : (
        <ul className="space-y-2">
          {pending.map((g) => (
            <li key={g.id} className="flex flex-wrap items-center gap-3 rounded-md border border-neutral-200 p-3 text-sm">
              <span className="font-medium">{g.subject_label}</span>
              <span className="text-xs text-neutral-500">{g.subject_kind}</span>
              <span className="text-xs text-neutral-500">Requested {fmtDate(g.created_at)}</span>
              {canOwn ? (
                <span className="ml-auto flex gap-2">
                  <Button
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
        </ul>
      )}
      <MutationError error={decide.error} />
      {history.length ? (
        <details>
          <summary className="cursor-pointer text-sm">History ({history.length})</summary>
          <ul aria-label="Reach history" className="divide-y divide-neutral-200 text-sm">
            {history.map((g) => (
              <li key={g.id} className="flex flex-wrap items-center gap-3 py-1.5">
                <span>{g.subject_label}</span>
                <StatusChip status={g.state} />
                <span className="text-xs text-neutral-500">
                  {g.decided_by ? `${g.decided_by.display_name} · ` : ""}
                  {fmtDate(g.decided_at ?? g.created_at)}
                </span>
              </li>
            ))}
          </ul>
        </details>
      ) : null}
    </section>
  );
}

export function AgentChannelsTab({ agentId, canOwn }: { agentId: string; orgId?: string; canOwn: boolean }) {
  return (
    <div className="space-y-6">
      <Presences agentId={agentId} />
      {canOwn ? <Bindings agentId={agentId} /> : null}
      <Reach agentId={agentId} canOwn={canOwn} />
    </div>
  );
}
