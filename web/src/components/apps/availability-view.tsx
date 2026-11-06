"use client";

import { useState } from "react";

import { ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
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
    <div className="space-y-4">
      <fieldset className="space-y-1" disabled={!canEdit}>
        <legend className="text-sm font-medium">Availability posture</legend>
        <label className="flex items-center gap-2 text-sm">
          <input type="radio" name="posture" checked={posture === "open"} onChange={() => setPosture("open")} />
          Open — every installed app can be connected
        </label>
        <label className="flex items-center gap-2 text-sm">
          <input type="radio" name="posture" checked={posture === "restricted"} onChange={() => setPosture("restricted")} />
          Restricted — only apps on the allow-list can be connected
        </label>
      </fieldset>
      {posture === "restricted" ? (
        <fieldset className="space-y-1" disabled={!canEdit}>
          <legend className="text-sm font-medium">Allowed apps (organization-wide)</legend>
          {apps.map((a) => (
            <label key={a.app_id} className="flex items-center gap-2 text-sm">
              <input type="checkbox" checked={orgWide(a.app_id)} onChange={() => setAllow((cur) => toggleAllow(cur, a.app_id, null))} />
              {a.name}
            </label>
          ))}
          {allow.filter((e) => e.workspace_id || !known.has(e.app_id)).length > 0 ? (
            <p className="text-xs text-neutral-500">
              Workspace-specific or unknown entries are kept as they are.
            </p>
          ) : null}
        </fieldset>
      ) : null}
      {put.error ? <p role="alert" className="text-sm text-red-700">{errorMessage(put.error)}</p> : null}
      {saved ? <p role="status" className="text-sm text-green-700">Saved.</p> : null}
      {canEdit ? (
        <Button size="sm" disabled={put.isPending} onClick={() => void save()}>Save</Button>
      ) : (
        <p className="text-sm text-neutral-600">Only owners and admins can change this.</p>
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
    <div className="max-w-2xl space-y-4">
      <h1 className="text-xl font-semibold">Apps</h1>
      <AvailabilityEditor orgId={orgId} initial={avail.data} apps={apps.data.items} canEdit={canEdit} />
    </div>
  );
}
