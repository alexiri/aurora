"use client";

import { useState, type FormEvent } from "react";

import { GrantForm } from "@/components/grants/grant-form";
import { ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import { Dialog, DialogClose, DialogContent } from "@/components/ui/dialog";
import { Notice } from "@/components/ui/notice";
import { useApp, appRestrictions, type Connection, type HasAppRestrictions } from "@/lib/api/apps";
import { usePutAgentConnectionGrant, type ConnectionGrant } from "@/lib/api/credential-grants";
import {
  grantValueBody,
  grantValueProblem,
  initialDecisions,
  initialGrantValue,
  inertToolIds,
  mapGrantError,
  toolsForMethod,
  type GrantErrors,
  type GrantValue,
  type Tool,
} from "@/lib/grant-editor";

/** The grant editor dialog of one agent's connection: `GrantForm` plus its save (`PUT`). */
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
        size="lg"
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
      tools={toolsForMethod(app.data.tools, connection.connect_method)}
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
  const [value, setValue] = useState<GrantValue>(() => initialGrantValue(tools, grant, restrictions.custom_only));
  const [errors, setErrors] = useState<GrantErrors>({ tools: {} });

  const inert = inertToolIds(tools, grant);
  const problem = grantValueProblem(value, customOnly);

  // An edit clears the error of what it touched (a row, the timeout field).
  const change = (next: GrantValue) => {
    setErrors((e) => {
      const rows = { ...e.tools };
      for (const id of Object.keys(next.decisions)) if (next.decisions[id] !== value.decisions[id]) rows[id] = "";
      return { ...e, tools: rows, timeout: next.timeout !== value.timeout ? undefined : e.timeout };
    });
    setValue(next);
  };

  function submit(e: FormEvent) {
    e.preventDefault();
    setErrors({ tools: {} });
    put.mutate(
      { connectionId: connection.id, body: grantValueBody(value, customOnly) },
      {
        onSuccess: onClose,
        onError: (err) => {
          const mapped = mapGrantError(err, tools, value.decisions);
          if (mapped.customOnly) {
            setLearnedCustomOnly(true);
            setValue({ mode: "custom", decisions: initialDecisions(tools, null), timeout: "" });
          }
          setErrors(mapped);
        },
      },
    );
  }

  return (
    <form onSubmit={submit} className="space-y-4" aria-label={`Access to ${connection.label}`}>
      {connection.access === "read" ? (
        <Notice tone="pending" role="status">
          Read-only connection: Aurora blocks every write tool through it, whatever you choose below. Change it on the
          connection.
        </Notice>
      ) : null}
      <GrantForm
        appName={appName}
        tools={tools}
        inert={inert}
        customOnly={customOnly}
        blockUntooled={restrictions.block_untooled}
        value={value}
        onChange={change}
        errors={errors}
        disabled={put.isPending}
      />
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
        <Button type="submit" disabled={put.isPending || problem !== null}>
          {put.isPending ? "Saving…" : "Save access"}
        </Button>
      </div>
    </form>
  );
}
