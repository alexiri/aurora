"use client";

import { useRouter } from "next/navigation";
import { useState } from "react";

import { AdminDeleteDialog } from "@/components/admin/admin-delete-dialog";
import { OrgDeleteConsequences } from "@/components/admin/orgs-view";
import { ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { DataTable, Td, Th } from "@/components/ui/data-table";
import { MoreNotShown } from "@/components/ui/more-not-shown";
import { Notice } from "@/components/ui/notice";
import { PageHeader } from "@/components/ui/page-header";
import { Section } from "@/components/ui/section";
import { adminRoutes } from "@/lib/routes";
import { useAdminDeleteOrg, useAdminDeleteWorkspace, useAdminOrg, useAdminOrgWorkspaces, type AdminOrgWorkspace } from "@/lib/api/admin-orgs";
import { fmtDate, plural } from "@/lib/utils";

function WorkspaceRow({ ws, onDeleted }: { ws: AdminOrgWorkspace; onDeleted: (msg: string) => void }) {
  const [open, setOpen] = useState(false);
  const del = useAdminDeleteWorkspace();
  return (
    <tr>
      <Td className="font-medium">{ws.name}</Td>
      <Td className="font-mono text-xs text-muted">{ws.slug}</Td>
      <Td>{ws.personal ? <Badge>Personal</Badge> : null}</Td>
      <Td className="text-right tabular-nums">{ws.agent_count}</Td>
      <Td className="whitespace-nowrap text-ink-2">{fmtDate(ws.created_at)}</Td>
      <Td className="text-right">
        <Button variant="outline" size="sm" onClick={() => setOpen(true)} aria-label={`Delete workspace ${ws.name}`}>
          Delete workspace…
        </Button>
        <AdminDeleteDialog
          open={open}
          onOpenChange={setOpen}
          title={`Delete ${ws.name}?`}
          noun="workspace"
          slug={ws.slug}
          consequences={
            <p>
              Deletes the workspace <strong className="text-ink">{ws.name}</strong> with its {plural(ws.agent_count, "agent")}.
              Conversations, credentials and settings of the workspace are deleted; the audit log and usage records are kept. This
              cannot be undone.
            </p>
          }
          run={(confirm) => del.mutateAsync({ wsId: ws.id, confirm })}
          onDone={() => onDeleted(`Deleted the workspace ${ws.name} (${ws.slug}).`)}
        />
      </Td>
    </tr>
  );
}

/** Admin › Organizations › one organization (P5-ADMDEL): counts and the workspace list, never content. */
export function OrgDetailView({ orgId }: { orgId: string }) {
  const router = useRouter();
  const org = useAdminOrg(orgId);
  const workspaces = useAdminOrgWorkspaces(orgId);
  const del = useAdminDeleteOrg();
  const [deleting, setDeleting] = useState(false);
  const [done, setDone] = useState<string | null>(null);
  if (org.isPending) return <Loading />;
  if (org.isError) return <ErrorState error={org.error} onRetry={() => org.refetch()} />;
  const o = org.data;
  return (
    <div className="max-w-page space-y-6">
      <PageHeader
        breadcrumb={[{ label: "Organizations", href: adminRoutes.organizations() }, { label: o.name }]}
        title={
          <span className="flex flex-wrap items-center gap-2">
            {o.name}
            {o.frozen ? <Badge tone="danger">Paused</Badge> : null}
            {o.ownerless ? <Badge>No owner</Badge> : null}
          </span>
        }
        description={
          <>
            <span className="font-mono text-xs">{o.slug}</span> · created {fmtDate(o.created_at)} · {plural(o.member_count, "active member")},{" "}
            {plural(o.workspace_count, "workspace")}, {plural(o.agent_count, "agent")}. You see counts and names, never the organization&apos;s
            data.
          </>
        }
        actions={
          <Button variant="danger" onClick={() => setDeleting(true)}>
            Delete organization…
          </Button>
        }
      />
      {done ? (
        <Notice tone="accent" role="status" data-testid="admin-orgs-done">
          {done}
        </Notice>
      ) : null}
      <Section id="org-workspaces" title="Workspaces">
        {workspaces.isPending ? (
          <Loading />
        ) : workspaces.isError ? (
          <ErrorState error={workspaces.error} onRetry={() => workspaces.refetch()} />
        ) : (
          <>
            {workspaces.data.truncated ? <MoreNotShown what="workspaces" /> : null}
            {workspaces.data.items.length === 0 ? (
              <p className="text-sm text-ink-2">This organization has no workspaces.</p>
            ) : (
              <DataTable caption={`Workspaces of ${o.name}`}>
                <thead>
                  <tr>
                    <Th>Name</Th>
                    <Th>Slug</Th>
                    <Th>Kind</Th>
                    <Th className="text-right">Agents</Th>
                    <Th>Created</Th>
                    <Th className="text-right">
                      <span className="sr-only">Actions</span>
                    </Th>
                  </tr>
                </thead>
                <tbody>
                  {workspaces.data.items.map((w) => (
                    <WorkspaceRow key={w.id} ws={w} onDeleted={setDone} />
                  ))}
                </tbody>
              </DataTable>
            )}
          </>
        )}
      </Section>
      <AdminDeleteDialog
        open={deleting}
        onOpenChange={setDeleting}
        title={`Delete ${o.name}?`}
        noun="organization"
        slug={o.slug}
        consequences={<OrgDeleteConsequences org={o} />}
        run={(confirm) => del.mutateAsync({ orgId, confirm })}
        onDone={() => router.push(adminRoutes.organizations())}
      />
    </div>
  );
}
