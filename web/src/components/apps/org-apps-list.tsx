"use client";

import { useRouter } from "next/navigation";
import { useState } from "react";

import { EmptyState } from "@/components/states";
import { AppIcon } from "@/components/ui/app-icon";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import { ItemList, ListRow } from "@/components/ui/section";
import { appDisplayName } from "@/lib/api/app-name";
import { connectDeepLink, useApps, type AppSummary } from "@/lib/api/apps";
import { useMe, useWorkspaces } from "@/lib/api/queries";
import { splitWorkspaces } from "@/lib/accounts";

import { AppLink } from "./app-link";
import { OrgConnectJourney } from "./connect-journey/org-journey";

/** One owned workspace in the Connect dialog: shows whether the app is available there (C-894). */
function WorkspaceChoice({
  orgSlug,
  workspace,
  appId,
  onChosen,
}: {
  orgSlug: string;
  workspace: { id: string; name: string; slug: string };
  appId: string;
  onChosen: () => void;
}) {
  const router = useRouter();
  const apps = useApps(workspace.id);
  const entry = apps.data?.items.find((a) => a.app_id === appId);
  const unavailable = apps.isSuccess && !entry?.available;
  return (
    <ListRow
      actions={
        <Button
          size="sm"
          variant="outline"
          disabled={apps.isPending || unavailable}
          aria-label={`Connect in ${workspace.name}`}
          onClick={() => {
            onChosen();
            router.push(connectDeepLink(orgSlug, workspace.slug, appId));
          }}
        >
          Choose
        </Button>
      }
    >
      <div className="min-w-0">
        <p className="truncate text-sm font-medium text-ink">{workspace.name}</p>
        {unavailable ? <p className="text-xs text-muted">Not available in this workspace.</p> : null}
      </div>
    </ListRow>
  );
}

/**
 * Where to connect (C-947, D-332): **For the whole organization** first (organization owners and admins), then the
 * person's own and shared workspaces, then other people's personal workspaces under a collapsed group.
 */
export function ConnectInWorkspaceDialog({
  orgId,
  orgSlug,
  app,
  canOrg = false,
  onOrg,
  onClose,
}: {
  orgId: string;
  orgSlug: string;
  app: Pick<AppSummary, "app_id" | "name" | "family">;
  /** The person is an organization owner or admin: the whole-organization choice is offered. */
  canOrg?: boolean;
  onOrg?: () => void;
  onClose: () => void;
}) {
  const workspaces = useWorkspaces(orgId);
  const me = useMe();
  const owned = (workspaces.data?.items ?? []).filter((w) => w.my_role === "owner");
  const { mine, others } = splitWorkspaces(owned, me.data?.user.id);
  const choice = (w: (typeof owned)[number]) => <WorkspaceChoice key={w.id} orgSlug={orgSlug} workspace={w} appId={app.app_id} onChosen={onClose} />;
  return (
    <Dialog open onOpenChange={(o) => (o ? undefined : onClose())}>
      <DialogContent title={`Connect ${appDisplayName(app)}`} description="Choose where to connect it.">
        {workspaces.isPending ? (
          <p className="text-sm text-ink-2">Loading…</p>
        ) : owned.length === 0 && !canOrg ? (
          <p className="text-sm text-ink-2">
            Only a workspace owner can connect apps. You are not an owner of any workspace in this organization: ask an
            owner to connect {appDisplayName(app)}.
          </p>
        ) : (
          <div className="space-y-3">
            <ItemList aria-label="Where to connect">
              {canOrg && onOrg ? (
                <ListRow
                  actions={
                    <Button size="sm" variant="outline" aria-label="Connect for the whole organization" onClick={onOrg}>
                      Choose
                    </Button>
                  }
                >
                  <div className="min-w-0">
                    <p className="truncate text-sm font-medium text-ink">For the whole organization</p>
                    <p className="text-xs text-muted">Any workspace&apos;s agents can be given it, within the limit you set.</p>
                  </div>
                </ListRow>
              ) : null}
              {mine.map(choice)}
            </ItemList>
            {others.length > 0 ? (
              <details>
                <summary className="cursor-pointer text-sm font-medium text-ink">Other people&apos;s personal workspaces ({others.length})</summary>
                <ItemList aria-label="Other people's personal workspaces" className="mt-2">
                  {others.map(choice)}
                </ItemList>
              </details>
            ) : null}
          </div>
        )}
      </DialogContent>
    </Dialog>
  );
}

/**
 * (FIX-153, C-894) The installed apps with **Connect…**, whatever the org posture. (P5-UX-9, C-933) Each name leads to
 * the app's setup page; with `switches` (the organization allows only some apps) each row has its on/off switch.
 */
export function OrgAppsList({
  orgId,
  orgSlug,
  apps,
  switches,
  canOrg = false,
}: {
  orgId: string;
  orgSlug: string;
  apps: AppSummary[];
  switches?: { isOn: (appId: string) => boolean; onToggle: (appId: string) => void; disabled?: boolean };
  /** Organization owners and admins may connect an app for the whole organization (C-936). */
  canOrg?: boolean;
}) {
  const [choosing, setChoosing] = useState<AppSummary | null>(null);
  const [forOrg, setForOrg] = useState<string | null>(null);
  if (apps.length === 0)
    return (
      <EmptyState title="No apps installed">
        Apps come from packages an instance admin approves.
      </EmptyState>
    );
  return (
    <>
      <ItemList aria-label="Installed apps">
        {apps.map((a) => (
          <ListRow
            key={a.app_id}
            actions={
              <>
                {switches ? (
                  <label className="inline-flex items-center gap-2 text-sm text-ink-2">
                    <input
                      type="checkbox"
                      className="size-4 accent-accent"
                      aria-label={appDisplayName(a)}
                      checked={switches.isOn(a.app_id)}
                      disabled={switches.disabled}
                      onChange={() => switches.onToggle(a.app_id)}
                    />
                    {switches.isOn(a.app_id) ? "On" : "Off"}
                  </label>
                ) : null}
                <Button size="sm" variant="outline" aria-label={`Connect ${appDisplayName(a)}`} onClick={() => setChoosing(a)}>
                  Connect…
                </Button>
              </>
            }
          >
            <AppIcon appId={a.app_id} name={appDisplayName(a)} hasDarkIcon={a.has_dark_icon} size={32} />
            <div className="min-w-0 flex-1">
              <p className="truncate text-sm font-medium text-ink">
                <AppLink app={a}>{appDisplayName(a)}</AppLink>
              </p>
              <p className="line-clamp-2 text-xs text-ink-2">{a.description}</p>
            </div>
          </ListRow>
        ))}
      </ItemList>
      {choosing ? (
        <ConnectInWorkspaceDialog
          orgId={orgId}
          orgSlug={orgSlug}
          app={choosing}
          canOrg={canOrg}
          onOrg={() => {
            setForOrg(choosing.app_id);
            setChoosing(null);
          }}
          onClose={() => setChoosing(null)}
        />
      ) : null}
      {forOrg ? <OrgConnectJourney orgId={orgId} appId={forOrg} onClose={() => setForOrg(null)} /> : null}
    </>
  );
}
