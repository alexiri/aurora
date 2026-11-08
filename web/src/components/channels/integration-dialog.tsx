"use client";

import { useState } from "react";

import { DynamicForm } from "@/components/apps/dynamic-form";
import { Markdown } from "@/components/apps/markdown";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import { defaultValues, validateFields, visibleFields, type FieldValues } from "@/lib/api/apps";
import { useCreateIntegration, usePatchIntegration, type ChannelIntegration } from "@/lib/api/channels";
import { formFields, type ConnectorInfo } from "@/lib/api/connectors";

import { MutationError, splitByStore } from "./shared";

/**
 * Create (or replace credentials of) an integration. Create is synchronous and can take
 * ~10 s while the connector validates, so the button shows a pending state. Credentials are
 * write-only: the form is never prefilled from the server.
 */
export function IntegrationDialog({
  orgId,
  connector,
  existing,
  onClose,
}: {
  orgId: string;
  connector: ConnectorInfo | null;
  existing?: ChannelIntegration;
  onClose: () => void;
}) {
  const fields = connector ? formFields(connector.integration_setup) : [];
  const [values, setValues] = useState<FieldValues>(() => defaultValues(fields));
  const [errors, setErrors] = useState<Record<string, string>>({});
  const create = useCreateIntegration(orgId);
  const patch = usePatchIntegration(orgId);
  const pending = create.isPending || patch.isPending;
  const error = create.error ?? patch.error;

  function submit(e: React.FormEvent) {
    e.preventDefault();
    if (!connector) return;
    const errs = validateFields(fields, values);
    // When replacing credentials, secret fields may stay empty ("stored").
    if (existing) for (const f of fields) if (f.store === "credentials") delete errs[f.name];
    setErrors(errs);
    if (Object.keys(errs).length) return;
    const visible = visibleFields(fields, values);
    const { config, credentials } = splitByStore(visible, values);
    if (existing) {
      patch.mutate(
        {
          id: existing.id,
          body: {
            config: { ...(existing.config as Record<string, unknown>), ...config } as Record<string, never>,
            ...(Object.keys(credentials).length ? { credentials: credentials as Record<string, never> } : {}),
          },
        },
        { onSuccess: onClose },
      );
    } else {
      create.mutate(
        {
          connector_id: connector.id,
          config: config as Record<string, never>,
          credentials: credentials as Record<string, never>,
        },
        { onSuccess: onClose },
      );
    }
  }

  return (
    <Dialog open={connector !== null} onOpenChange={(o) => !o && !pending && onClose()}>
      <DialogContent
        title={`${existing ? "Update" : "Connect"} ${connector?.display_name ?? ""}`}
        description="Credentials are stored securely and are never shown again."
      >
        {connector ? (
          <form className="space-y-3" onSubmit={submit} aria-busy={pending}>
            {connector.setup_guide_md ? (
              <div className="rounded-control border border-line bg-subtle p-3 text-sm text-ink-2">
                <Markdown source={connector.setup_guide_md} />
              </div>
            ) : null}
            <DynamicForm
              fields={fields}
              values={values}
              errors={errors}
              onChange={(n, v) => setValues((p) => ({ ...p, [n]: v }))}
            />
            {existing ? (
              <p className="text-xs text-muted">Credentials: stored. Leave blank to keep them.</p>
            ) : null}
            <MutationError error={error} />
            {pending ? (
              <p role="status" className="text-sm text-ink-2">
                Validating with {connector.display_name}… this can take up to 10 seconds.
              </p>
            ) : null}
            <Button type="submit" size="lg" disabled={pending}>
              {pending ? "Validating…" : existing ? "Save" : "Connect"}
            </Button>
          </form>
        ) : null}
      </DialogContent>
    </Dialog>
  );
}
