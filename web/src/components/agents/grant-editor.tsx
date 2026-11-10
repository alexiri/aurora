"use client";

import { useState, type FormEvent } from "react";

import { ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogClose, DialogContent } from "@/components/ui/dialog";
import { Field, inputClass } from "@/components/ui/field";
import { Notice } from "@/components/ui/notice";
import { Segmented } from "@/components/ui/segmented";
import { useApp, appRestrictions, type Connection, type HasAppRestrictions } from "@/lib/api/apps";
import { usePutAgentConnectionGrant, type ConnectionGrant } from "@/lib/api/credential-grants";
import {
  ASK_TIMEOUT_MAX,
  ASK_TIMEOUT_MIN,
  GROUPS,
  counts,
  customBody,
  groupOf,
  groupState,
  groupTools,
  initialDecisions,
  inertToolIds,
  mapGrantError,
  parseTimeout,
  setGroup,
  timeoutProblem,
  type Decision,
  type GrantErrors,
  type Group,
  type Tool,
} from "@/lib/grant-editor";

const DECISIONS = [
  { value: "allow", label: "Allow" },
  { value: "ask", label: "Ask" },
  { value: "block", label: "Block" },
] as const;

type Mode = "full" | "custom";

/** The grant editor: Full access or Custom (Allow / Ask / Block per tool, grouped Read and Write). */
export function GrantEditor({
  agentId,
  wsId,
  connection,
  grant,
  restrictions,
  open,
  onOpenChange,
}: {
  agentId: string;
  /** The workspace the agent lives in: an organization connection has none of its own, the app detail needs one. */
  wsId: string;
  connection: Connection;
  grant: ConnectionGrant | null;
  /** The grant (or any item) carrying `app_restrictions`, as the API sends it (C-410). */
  restrictions?: HasAppRestrictions | null;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        title={`Access to ${connection.label}`}
        description="What this agent may do through the connection."
        className="w-[min(40rem,calc(100vw-2rem))]"
      >
        {open ? (
          <EditorLoader agentId={agentId} wsId={wsId} connection={connection} grant={grant} restrictions={restrictions} onClose={() => onOpenChange(false)} />
        ) : null}
      </DialogContent>
    </Dialog>
  );
}

function EditorLoader({
  agentId,
  wsId,
  connection,
  grant,
  restrictions,
  onClose,
}: {
  agentId: string;
  wsId: string;
  connection: Connection;
  grant: ConnectionGrant | null;
  restrictions?: HasAppRestrictions | null;
  onClose: () => void;
}) {
  const app = useApp(connection.app_id, connection.workspace_id ?? wsId);
  if (app.isPending) return <Loading />;
  if (app.isError) return <ErrorState error={app.error} onRetry={() => app.refetch()} />;
  return (
    <EditorForm
      agentId={agentId}
      connection={connection}
      grant={grant}
      tools={app.data.tools}
      appName={app.data.name}
      restrictions={appRestrictions(restrictions, app.data)}
      onClose={onClose}
    />
  );
}

function EditorForm({
  agentId,
  connection,
  grant,
  tools,
  appName,
  restrictions,
  onClose,
}: {
  agentId: string;
  connection: Connection;
  grant: ConnectionGrant | null;
  tools: Tool[];
  appName: string;
  restrictions: { custom_only: boolean; block_untooled: boolean };
  onClose: () => void;
}) {
  const put = usePutAgentConnectionGrant(agentId);
  // `custom_only` from `app_restrictions`, or learned from a 422 `full_grant_forbidden` on save.
  const [learnedCustomOnly, setLearnedCustomOnly] = useState(false);
  const customOnly = restrictions.custom_only || learnedCustomOnly;
  const [mode, setMode] = useState<Mode>(grant?.mode === "custom" || restrictions.custom_only ? "custom" : "full");
  const [decisions, setDecisions] = useState<Record<string, Decision>>(() => initialDecisions(tools, grant));
  const [timeout, setTimeoutText] = useState(grant?.ask_timeout_secs != null ? String(grant.ask_timeout_secs) : "");
  const [errors, setErrors] = useState<GrantErrors>({ tools: {} });

  const inert = inertToolIds(tools, grant);
  const anyAsk = counts(decisions).ask > 0;
  const timeoutHint = anyAsk ? timeoutProblem(timeout) : undefined;
  const effectiveMode: Mode = customOnly ? "custom" : mode;

  const choose = (id: string, d: Decision) => {
    setDecisions((cur) => ({ ...cur, [id]: d }));
    setErrors((e) => ({ ...e, tools: { ...e.tools, [id]: "" } }));
  };

  function submit(e: FormEvent) {
    e.preventDefault();
    setErrors({ tools: {} });
    const body = effectiveMode === "full" ? { mode: "full" as const } : customBody(decisions, timeout);
    put.mutate(
      { connectionId: connection.id, body },
      {
        onSuccess: onClose,
        onError: (err) => {
          const mapped = mapGrantError(err, tools, decisions);
          if (mapped.customOnly) {
            setLearnedCustomOnly(true);
            setMode("custom");
            setDecisions(initialDecisions(tools, null));
          }
          setErrors(mapped);
        },
      },
    );
  }

  return (
    <form onSubmit={submit} className="space-y-4" aria-label={`Access to ${connection.label}`}>
      {customOnly ? (
        <Notice tone="pending" role="status">
          This app only supports custom access: choose the tools the agent may use.
        </Notice>
      ) : (
        <div className="space-y-1.5">
          <p className="text-sm font-medium text-ink">
            Access to {appName}
          </p>
          <Segmented<Mode>
            label="Access mode"
            value={mode}
            onChange={(m) => {
              setMode(m);
              if (m === "custom" && grant?.mode !== "custom") setDecisions(initialDecisions(tools, null));
            }}
            options={[
              { value: "full", label: "Full access" },
              { value: "custom", label: "Custom" },
            ]}
          />
          {mode === "full" ? (
            <p className="text-xs text-muted">Every tool of the app, including tools added by later updates.</p>
          ) : null}
        </div>
      )}
      {restrictions.block_untooled ? (
        <Notice role="status">Requests that are not in the tool list are always blocked.</Notice>
      ) : null}

      {effectiveMode === "custom" ? (
        <div className="space-y-4">
          <p className="text-xs text-muted">
            Allow runs the tool, Ask holds it for an approval, Block refuses it. A tool you do not list stays blocked.
          </p>
          {GROUPS.map((g) => (
            <ToolGroup
              key={g.value}
              group={g.value}
              label={g.label}
              tools={tools}
              decisions={decisions}
              errors={errors.tools}
              disabled={put.isPending}
              onChoose={choose}
              onSetAll={(d) => setDecisions((cur) => setGroup(cur, tools, g.value, d))}
            />
          ))}
          {inert.length > 0 ? (
            <section aria-label="Inert tools" className="space-y-1.5">
              <h3 className="text-sm font-semibold text-ink">Inert tools</h3>
              <p className="text-xs text-muted">
                An app update removed these tools. They grant nothing; set them to Block to drop them from this grant.
              </p>
              <ul className="divide-y divide-line rounded-control border border-line">
                {inert.map((id) => (
                  <li key={id} className="space-y-1.5 px-3 py-2">
                    <div className="flex flex-wrap items-center gap-2">
                      <code className="min-w-0 break-all text-sm text-ink">{id}</code>
                      <Badge tone="pending">Inert</Badge>
                    </div>
                    {errors.tools[id] ? (
                      <p role="alert" className="text-xs text-danger-ink">
                        {errors.tools[id]}
                      </p>
                    ) : null}
                    <Segmented<Decision>
                      label={`Access for ${id}`}
                      value={decisions[id] ?? "block"}
                      options={[...DECISIONS]}
                      onChange={(d) => choose(id, d)}
                      disabled={put.isPending}
                    />
                  </li>
                ))}
              </ul>
            </section>
          ) : null}
          {anyAsk ? (
            <Field
              id="ge-timeout"
              label="Approval timeout (seconds)"
              hint={`Empty = the instance default (5 min). From ${ASK_TIMEOUT_MIN} to ${ASK_TIMEOUT_MAX} s, at most the instance maximum (1 h unless raised).`}
              error={errors.timeout ?? timeoutHint}
            >
              <input
                id="ge-timeout"
                inputMode="numeric"
                placeholder="default"
                className={inputClass + " w-32"}
                value={timeout}
                onChange={(e) => {
                  setTimeoutText(e.target.value);
                  setErrors((x) => ({ ...x, timeout: undefined }));
                }}
                disabled={put.isPending}
              />
            </Field>
          ) : null}
        </div>
      ) : null}

      {errors.general ? (
        <Notice tone="danger" role="alert">
          {errors.general}
        </Notice>
      ) : null}
      {put.isError && !errors.general && !errors.timeout && !errors.customOnly && Object.values(errors.tools).every((v) => !v) ? (
        <ErrorState error={put.error} />
      ) : null}

      <div className="flex flex-wrap justify-end gap-2">
        <DialogClose asChild>
          <Button type="button" variant="outline">
            Cancel
          </Button>
        </DialogClose>
        <Button type="submit" disabled={put.isPending || (effectiveMode === "custom" && anyAsk && (timeoutHint !== undefined || parseTimeout(timeout) === "invalid"))}>
          {put.isPending ? "Saving…" : "Save access"}
        </Button>
      </div>
    </form>
  );
}

function ToolGroup({
  group,
  label,
  tools,
  decisions,
  errors,
  disabled,
  onChoose,
  onSetAll,
}: {
  group: Group;
  label: string;
  tools: Tool[];
  decisions: Record<string, Decision>;
  errors: Record<string, string>;
  disabled: boolean;
  onChoose: (id: string, d: Decision) => void;
  onSetAll: (d: Decision) => void;
}) {
  const rows = groupTools(tools, group);
  if (rows.length === 0) return null;
  const state = groupState(decisions, tools, group);
  return (
    <section aria-label={`${label} tools`} className="space-y-1.5">
      <div className="flex flex-wrap items-center justify-between gap-x-3 gap-y-1.5">
        <h3 className="text-sm font-semibold text-ink">{label}</h3>
        <Segmented<Decision>
          label={`Set all ${label.toLowerCase()} tools`}
          value={state === "mixed" || state === null ? null : state}
          options={[...DECISIONS]}
          onChange={onSetAll}
          disabled={disabled}
        />
      </div>
      <ul className="divide-y divide-line rounded-control border border-line">
        {rows.map((t) => {
          const err = errors[t.id];
          return (
            <li key={t.id} className="space-y-1.5 px-3 py-2">
              <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
                <span className="min-w-0 break-words text-sm font-medium text-ink">{t.name}</span>
                {t.wildcard ? <Badge>Umbrella</Badge> : null}
              </div>
              <p className="break-all text-xs text-muted">
                {t.wildcard
                  ? `Covers the whole ${groupOf(t)} group of ${t.host === "*" ? "the app" : t.host}.`
                  : `${t.methods.join(", ")} ${t.host}${t.path}`}
              </p>
              {err ? (
                <p role="alert" className="text-xs text-danger-ink">
                  {err}
                </p>
              ) : null}
              <Segmented<Decision>
                label={`Access for ${t.name}`}
                value={decisions[t.id] ?? "block"}
                options={[...DECISIONS]}
                onChange={(d) => onChoose(t.id, d)}
                disabled={disabled}
              />
            </li>
          );
        })}
      </ul>
    </section>
  );
}
