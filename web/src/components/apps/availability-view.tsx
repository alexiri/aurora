"use client";

import Link from "next/link";
import { useState } from "react";

import { ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import { CheckboxField, FormActions } from "@/components/ui/form";
import { Notice } from "@/components/ui/notice";
import { PageHeader } from "@/components/ui/page-header";
import { errorMessage } from "@/lib/api/errors";
import {
  toggleAllow,
  useApps,
  useAppAvailability,
  usePutAppAvailability,
  type AppAvailability,
} from "@/lib/api/apps";

/**
 * The per-app availability list (which apps are available when the org posture is `restricted`). The posture
 * itself lives on Org › Governance (FIX-130): while it is `open` this page only says so and links there. A save
 * sends the posture as it currently is on the server together with the list.
 */
export function AvailabilityEditor({
  orgId,
  orgSlug,
  initial,
  apps,
  canEdit,
}: {
  orgId: string;
  orgSlug: string;
  initial: AppAvailability;
  apps: { app_id: string; name: string }[];
  canEdit: boolean;
}) {
  const put = usePutAppAvailability(orgId);
  const live = useAppAvailability(orgId);
  const posture = live.data?.posture ?? initial.posture;
  const [allow, setAllow] = useState(initial.allow);
  const [saved, setSaved] = useState(false);
  const orgWide = (id: string) => allow.some((e) => e.app_id === id && !e.workspace_id);
  const known = new Set(apps.map((a) => a.app_id));
  async function save() {
    setSaved(false);
    try {
      await put.mutateAsync({ posture: posture === "restricted" ? "restricted" : "open", allow });
      setSaved(true);
    } catch {
      // shown below
    }
  }
  if (posture !== "restricted") {
    return (
      <Notice role="status">
        Every app is available — change in{" "}
        <Link className="font-medium underline" href={`/o/${orgSlug}/governance`}>
          Governance
        </Link>
        .
      </Notice>
    );
  }
  return (
    <div className="space-y-5">
      <fieldset className="min-w-0 space-y-3 rounded-card border border-line bg-surface p-4 sm:p-5" disabled={!canEdit}>
        <legend className="px-1 font-display text-base font-semibold text-ink">Allowed apps (organization-wide)</legend>
        <p className="text-sm text-ink-2">
          Only these apps can be connected while the posture is restricted (set in{" "}
          <Link className="font-medium text-link underline" href={`/o/${orgSlug}/governance`}>
            Governance
          </Link>
          ).
        </p>
        {apps.map((a) => (
          <CheckboxField
            key={a.app_id}
            id={`allow-${a.app_id}`}
            label={a.name}
            checked={orgWide(a.app_id)}
            onChange={() => setAllow((cur) => toggleAllow(cur, a.app_id, null))}
          />
        ))}
        {allow.filter((e) => e.workspace_id || !known.has(e.app_id)).length > 0 ? (
          <p className="text-xs text-muted">Workspace-specific or unknown entries are kept as they are.</p>
        ) : null}
      </fieldset>
      {put.error ? (
        <Notice tone="danger" role="alert">
          {errorMessage(put.error)}
        </Notice>
      ) : null}
      {saved ? (
        <Notice tone="accent" role="status">
          Saved.
        </Notice>
      ) : null}
      {canEdit ? (
        <FormActions>
          <Button size="lg" disabled={put.isPending} onClick={() => void save()}>
            Save
          </Button>
        </FormActions>
      ) : (
        <Notice role="status">Only owners and admins can change this.</Notice>
      )}
    </div>
  );
}

export function AvailabilityView({ orgId, orgSlug, canEdit }: { orgId: string; orgSlug: string; canEdit: boolean }) {
  const avail = useAppAvailability(orgId);
  const apps = useApps(undefined);
  if (avail.isPending || apps.isPending) return <Loading />;
  if (avail.isError) return <ErrorState error={avail.error} onRetry={() => avail.refetch()} />;
  if (apps.isError) return <ErrorState error={apps.error} onRetry={() => apps.refetch()} />;
  return (
    <div className="max-w-page-narrow">
      <PageHeader title="Apps" description="Choose which installed apps can be connected in this organization." />
      <AvailabilityEditor orgId={orgId} orgSlug={orgSlug} initial={avail.data} apps={apps.data.items} canEdit={canEdit} />
    </div>
  );
}
