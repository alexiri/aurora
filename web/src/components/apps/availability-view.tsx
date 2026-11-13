"use client";

import { useState } from "react";

import { ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import { CheckboxField, FormActions, FormSection } from "@/components/ui/form";
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

export function AvailabilityEditor({
  orgId,
  initial,
  apps,
  canEdit,
}: {
  orgId: string;
  initial: AppAvailability;
  apps: { app_id: string; name: string }[];
  canEdit: boolean;
}) {
  const put = usePutAppAvailability(orgId);
  const [posture, setPosture] = useState<"open" | "restricted">(initial.posture === "restricted" ? "restricted" : "open");
  const [allow, setAllow] = useState(initial.allow);
  const [saved, setSaved] = useState(false);
  const orgWide = (id: string) => allow.some((e) => e.app_id === id && !e.workspace_id);
  const known = new Set(apps.map((a) => a.app_id));
  async function save() {
    setSaved(false);
    try {
      await put.mutateAsync({ posture, allow });
      setSaved(true);
    } catch {
      // shown below
    }
  }
  return (
    <div className="space-y-5">
      <FormSection title="Availability posture" description="Which installed apps people in this organization can connect.">
        <fieldset className="space-y-3" disabled={!canEdit}>
          <legend className="sr-only">Posture options</legend>
          <label className="flex items-start gap-2.5 text-sm text-ink">
            <input type="radio" name="posture" className="mt-0.5 size-4 accent-accent" checked={posture === "open"} onChange={() => setPosture("open")} />
            Open — every installed app can be connected
          </label>
          <label className="flex items-start gap-2.5 text-sm text-ink">
            <input type="radio" name="posture" className="mt-0.5 size-4 accent-accent" checked={posture === "restricted"} onChange={() => setPosture("restricted")} />
            Restricted — only apps on the allow-list can be connected
          </label>
        </fieldset>
      </FormSection>
      {posture === "restricted" ? (
        <fieldset className="min-w-0 space-y-3 rounded-card border border-line bg-surface p-4 sm:p-5" disabled={!canEdit}>
          <legend className="px-1 font-display text-base font-semibold text-ink">Allowed apps (organization-wide)</legend>
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
      ) : null}
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

export function AvailabilityView({ orgId, canEdit }: { orgId: string; canEdit: boolean }) {
  const avail = useAppAvailability(orgId);
  const apps = useApps(undefined);
  if (avail.isPending || apps.isPending) return <Loading />;
  if (avail.isError) return <ErrorState error={avail.error} onRetry={() => avail.refetch()} />;
  if (apps.isError) return <ErrorState error={apps.error} onRetry={() => apps.refetch()} />;
  return (
    <div className="max-w-page-narrow">
      <PageHeader title="Apps" description="Choose which installed apps can be connected in this organization." />
      <AvailabilityEditor orgId={orgId} initial={avail.data} apps={apps.data.items} canEdit={canEdit} />
    </div>
  );
}
