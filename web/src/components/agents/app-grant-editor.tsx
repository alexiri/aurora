"use client";

import { useState, type FormEvent } from "react";

import { GrantForm } from "@/components/grants/grant-form";
import { ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import { Dialog, DialogClose, DialogContent } from "@/components/ui/dialog";
import { Notice } from "@/components/ui/notice";
import { appDisplayName } from "@/lib/api/app-name";
import { appRestrictions, useApp, type AppSummary } from "@/lib/api/apps";
import { useGrantApp } from "@/lib/api/credential-grants";
import {
  grantValueBody,
  grantValueProblem,
  initialDecisions,
  initialGrantValue,
  mapGrantError,
  type GrantErrors,
  type GrantValue,
  type Tool,
} from "@/lib/grant-editor";

/**
 * (FIX-154, C-897) The grant editor for a credential-free app that has no connection yet: the same `GrantForm`, saved
 * through `PUT …/grants/apps/{app}`, which creates (or reuses) the workspace's connection in the same step.
 */
export function AppGrantEditor({
  agentId,
  wsId,
  app,
  onClose,
}: {
  agentId: string;
  wsId: string;
  app: AppSummary;
  onClose: () => void;
}) {
  const name = appDisplayName(app);
  return (
    <Dialog open onOpenChange={(o) => (o ? undefined : onClose())}>
      <DialogContent title={`Access to ${name}`} description="No sign-in needed. What this agent may do with the app." size="lg">
        <Loader agentId={agentId} wsId={wsId} app={app} onClose={onClose} />
      </DialogContent>
    </Dialog>
  );
}

function Loader({ agentId, wsId, app, onClose }: { agentId: string; wsId: string; app: AppSummary; onClose: () => void }) {
  const detail = useApp(app.app_id, wsId);
  if (detail.isPending) return <Loading />;
  if (detail.isError) return <ErrorState error={detail.error} onRetry={() => detail.refetch()} />;
  return (
    <Form
      agentId={agentId}
      app={app}
      appName={appDisplayName(detail.data)}
      tools={detail.data.tools as Tool[]}
      restrictions={appRestrictions(null, detail.data)}
      onClose={onClose}
    />
  );
}

function Form({
  agentId,
  app,
  appName,
  tools,
  restrictions,
  onClose,
}: {
  agentId: string;
  app: AppSummary;
  appName: string;
  tools: Tool[];
  restrictions: { custom_only: boolean; block_untooled: boolean };
  onClose: () => void;
}) {
  const put = useGrantApp(agentId);
  // `custom_only` from `app_restrictions`, or learned from a 422 `full_grant_forbidden` on save.
  const [learnedCustomOnly, setLearnedCustomOnly] = useState(false);
  const customOnly = restrictions.custom_only || learnedCustomOnly;
  // Full is preselected unless the app is custom-only (`initialGrantValue`).
  const [value, setValue] = useState<GrantValue>(() => initialGrantValue(tools, null, restrictions.custom_only));
  const [errors, setErrors] = useState<GrantErrors>({ tools: {} });
  const problem = grantValueProblem(value, customOnly);

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
      { appId: app.app_id, body: grantValueBody(value, customOnly) },
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
    <form onSubmit={submit} className="space-y-4" aria-label={`Access to ${appName}`}>
      <GrantForm
        appName={appName}
        tools={tools}
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
