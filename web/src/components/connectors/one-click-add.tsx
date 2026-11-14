"use client";

import { useState } from "react";

import { DynamicForm } from "@/components/apps/dynamic-form";
import { Button } from "@/components/ui/button";
import { Notice } from "@/components/ui/notice";
import { ApiError } from "@/lib/api/client";
import {
  setupFields,
  useCreateManagedConnector,
  type ConnectorType,
  type CreateConnectorRequest,
} from "@/lib/api/connectors";

import { shortDescription } from "./short-description";
import { ManifestSummary } from "./manifest-views";
import { ConnectorError, ConnectorIcon, UnsignedBadge } from "./shared";
import { splitSettings } from "./setup-values";

/**
 * (FIX-127, D-219) A type with nothing required: the manifest summary is inline, the settings
 * (all optional or generated) are collapsed under "Advanced", and **Add** approves exactly the
 * digest shown here and creates the connector. Never automatic: adding exposes public callback
 * endpoints and approves a manifest, so it stays an explicit instance-admin act.
 */
export function OneClickRow({
  type,
  onAdded,
  onStale,
}: {
  type: ConnectorType;
  onAdded: (id: string) => void;
  onStale: () => void;
}) {
  const create = useCreateManagedConnector();
  const fields = setupFields(type.instance_setup);
  const [values, setValues] = useState<Record<string, string>>({});
  const [stale, setStale] = useState(false);

  function add() {
    const { settings, secrets } = splitSettings(fields, values);
    const body: CreateConnectorRequest = {
      id: type.type_id,
      approve_manifest_digest: type.manifest_digest,
      settings: settings as Record<string, never>,
      secrets,
    };
    create.mutate(body, {
      onSuccess: (r) => onAdded(r.connector.id),
      onError: (e) => {
        if (e instanceof ApiError && e.code === "stale_manifest") {
          setStale(true);
          onStale();
        }
      },
    });
  }

  const staleError = create.error instanceof ApiError && create.error.code === "stale_manifest";
  return (
    <li className="space-y-3 p-3 sm:px-4" aria-label={type.name}>
      <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
        <ConnectorIcon id={type.type_id} name={type.name} iconB64={type.icon_png_b64} hasIcon={false} />
        <div className="min-w-0 flex-1 basis-48">
          <p className="flex flex-wrap items-center gap-x-2 text-sm font-semibold text-ink">
            {type.name}
            <span className="text-xs font-normal text-muted">
              v{type.version} · {type.publisher}
            </span>
            {type.signed === false ? <UnsignedBadge /> : null}
          </p>
          {shortDescription(type.setup_guide_md) ? (
            <p className="text-sm text-ink-2">{shortDescription(type.setup_guide_md)}</p>
          ) : null}
          <p className="text-xs text-muted">Nothing to fill in: adding approves what is listed below.</p>
        </div>
        <Button aria-label={`Add ${type.name}`} disabled={create.isPending} onClick={add}>
          {create.isPending ? "Adding…" : "Add"}
        </Button>
      </div>
      <details className="rounded-control border border-line bg-subtle p-3 text-sm">
        <summary className="cursor-pointer font-medium text-ink">What {type.name} can do</summary>
        <div className="mt-3">
          <ManifestSummary manifest={type.manifest} />
        </div>
      </details>
      {fields.length > 0 ? (
        <details className="rounded-control border border-line p-3 text-sm">
          <summary className="cursor-pointer font-medium text-ink">Advanced</summary>
          <div className="mt-3 space-y-3">
            <DynamicForm
              fields={fields}
              values={values}
              errors={{}}
              onChange={(name, v) => setValues((p) => ({ ...p, [name]: v }))}
            />
          </div>
        </details>
      ) : null}
      {stale || staleError ? (
        <Notice tone="pending" role="status">
          The connector type changed while you were adding it. The latest version is shown; check it and add again.
        </Notice>
      ) : create.error ? (
        <ConnectorError error={create.error} />
      ) : null}
    </li>
  );
}
