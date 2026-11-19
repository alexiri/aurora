"use client";

import { useState } from "react";

import { ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { FormActions } from "@/components/ui/form";
import { ErrorNotice, Notice, SavedNotice } from "@/components/ui/notice";
import { PageHeader } from "@/components/ui/page-header";
import {
  toggleAllow,
  useApps,
  useAppAvailability,
  usePutAppAvailability,
  type AppAvailability,
} from "@/lib/api/apps";

import { OrgAppsList } from "./org-apps-list";

/**
 * (P5-UX-9, C-933) Which apps people in the organization can connect, and the list of apps, on one page: the choice
 * (every app, or only the ones switched on below) above the list, with one Save. A save sends the choice and the list
 * together (`PUT /v1/orgs/{org}/app-availability`); entries for one workspace, and for apps this page does not list, are
 * kept as they are.
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
  apps: Parameters<typeof OrgAppsList>[0]["apps"];
  canEdit: boolean;
}) {
  const put = usePutAppAvailability(orgId);
  const [posture, setPosture] = useState<"open" | "restricted">(initial.posture === "restricted" ? "restricted" : "open");
  const [allow, setAllow] = useState(initial.allow);
  const [saved, setSaved] = useState(false);
  const orgWide = (id: string) => allow.some((e) => e.app_id === id && !e.workspace_id);
  const known = new Set(apps.map((a) => a.app_id));
  const restricted = posture === "restricted";
  const choose = (p: "open" | "restricted") => {
    setPosture(p);
    setSaved(false);
  };
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
      <Card className="space-y-3" id="availability">
        <fieldset className="min-w-0 space-y-3" disabled={!canEdit}>
          <legend className="font-display text-base font-semibold text-ink">Which apps can people connect?</legend>
          <label className="flex items-start gap-2.5 text-sm text-ink">
            <input
              type="radio"
              name="app-posture"
              className="mt-0.5 size-4 accent-accent"
              checked={!restricted}
              onChange={() => choose("open")}
            />
            People can connect every app
          </label>
          <label className="flex items-start gap-2.5 text-sm text-ink">
            <input
              type="radio"
              name="app-posture"
              className="mt-0.5 size-4 accent-accent"
              checked={restricted}
              onChange={() => choose("restricted")}
            />
            People can connect only the apps switched on below
          </label>
        </fieldset>
      </Card>
      <OrgAppsList
        orgId={orgId}
        orgSlug={orgSlug}
        canOrg={canEdit}
        apps={apps}
        switches={
          restricted
            ? {
                isOn: (id) => orgWide(id),
                onToggle: (id) => {
                  setAllow((cur) => toggleAllow(cur, id, null));
                  setSaved(false);
                },
                disabled: !canEdit,
              }
            : undefined
        }
      />
      {restricted && allow.filter((e) => e.workspace_id || !known.has(e.app_id)).length > 0 ? (
        <p className="text-xs text-muted">Workspace-specific or unknown entries are kept as they are.</p>
      ) : null}
      <ErrorNotice error={put.error} />
      <SavedNotice show={saved}>Saved.</SavedNotice>
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
      <PageHeader
        title="Apps"
        description="The installed apps, and which of them people in this organization can connect."
      />
      <AvailabilityEditor orgId={orgId} orgSlug={orgSlug} initial={avail.data} apps={apps.data.items} canEdit={canEdit} />
    </div>
  );
}
