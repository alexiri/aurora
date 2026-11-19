"use client";

import Link from "next/link";
import { useState, type FormEvent } from "react";

import { AdminDeleteDialog } from "@/components/admin/admin-delete-dialog";
import { ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { FormDialog } from "@/components/ui/confirm-dialog";
import { DataTable, Td, Th } from "@/components/ui/data-table";
import { Field, inputClass } from "@/components/ui/field";
import { CheckboxField } from "@/components/ui/form";
import { MoreIcon } from "@/components/ui/icons";
import { Menu, MenuContent, MenuItem, MenuSeparator, MenuTrigger } from "@/components/ui/menu";
import { MoreNotShown } from "@/components/ui/more-not-shown";
import { ErrorNotice, Notice } from "@/components/ui/notice";
import { PageHeader } from "@/components/ui/page-header";
import { useAdminDeleteOrg } from "@/lib/api/admin-orgs";
import { errorMessage } from "@/lib/api/errors";
import { adminRoutes } from "@/lib/routes";
import { useAdminFreezeOrg, useAdminOrgs, type AdminOrg } from "@/lib/api/freeze";
import { fmtDate, plural } from "@/lib/utils";

/** What an organization delete removes, with the counts (docs/14 §Instance administration). */
export function OrgDeleteConsequences({ org }: { org: Pick<AdminOrg, "name" | "workspace_count" | "agent_count" | "member_count"> }) {
  return (
    <p>
      Deletes the organization <strong className="text-ink">{org.name}</strong> with its{" "}
      {plural(org.workspace_count, "workspace")}, {plural(org.agent_count, "agent")} and {plural(org.member_count, "member")}
      {org.member_count === 1 ? "’s" : "’"} access. Conversations, credentials and settings are deleted; the audit log and usage records are kept. This cannot be undone.
    </p>
  );
}

/** Freeze / Unfreeze as the operator: the freeze asks for a reason, the lift is one click. */
function FreezeDialog({ org, open, onOpenChange }: { org: AdminOrg; open: boolean; onOpenChange: (o: boolean) => void }) {
  const m = useAdminFreezeOrg(org.id);
  const [reason, setReason] = useState("");
  async function submit(e: FormEvent) {
    e.preventDefault();
    try {
      await m.freeze.mutateAsync(reason.trim());
      setReason("");
      onOpenChange(false);
    } catch {
      // shown through m.freeze.error
    }
  }
  return (
    <FormDialog
      open={open}
      onOpenChange={onOpenChange}
      title={`Freeze ${org.name}?`}
      description="Stops every agent of every workspace until an instance administrator or the organization's owner lifts it. Nothing is deleted, and you gain no access to the organization's data."
      submitLabel={m.freeze.isPending ? "Freezing…" : "Freeze organization"}
      danger
      busy={m.freeze.isPending}
      submitDisabled={reason.trim() === ""}
      onSubmit={(e) => void submit(e)}
    >
      <Field id={`freeze-reason-${org.id}`} label="Reason" hint="Shown to the organization's owners.">
        <input id={`freeze-reason-${org.id}`} className={inputClass} value={reason} onChange={(e) => setReason(e.target.value)} />
      </Field>
      <ErrorNotice error={m.freeze.error} />
    </FormDialog>
  );
}

function OrgRowActions({ org, onDeleted, onError }: { org: AdminOrg; onDeleted: (msg: string) => void; onError: (msg: string) => void }) {
  const [freezing, setFreezing] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const freeze = useAdminFreezeOrg(org.id);
  const del = useAdminDeleteOrg();
  return (
    <>
      {/* Non-modal: a modal menu aria-hides the rest of the page while its controls stay reachable (axe: aria-hidden-focus). */}
      <Menu modal={false}>
        <MenuTrigger asChild>
          <Button variant="ghost" size="sm" aria-label={`Actions for ${org.name}`} className="size-11 min-h-11 px-0 md:size-9 md:min-h-9">
            <MoreIcon width={20} height={20} />
          </Button>
        </MenuTrigger>
        <MenuContent align="end">
          {org.frozen ? (
            <MenuItem onSelect={() => void freeze.unfreeze.mutateAsync().catch((e) => onError(errorMessage(e)))}>Unfreeze</MenuItem>
          ) : (
            <MenuItem onSelect={() => setFreezing(true)}>Freeze…</MenuItem>
          )}
          <MenuItem asChild>
            <Link href={adminRoutes.organization(org.id)}>Workspaces</Link>
          </MenuItem>
          <MenuSeparator />
          <MenuItem className="text-danger-ink data-[highlighted]:bg-danger-soft" onSelect={() => setDeleting(true)}>
            Delete organization…
          </MenuItem>
        </MenuContent>
      </Menu>
      <FreezeDialog org={org} open={freezing} onOpenChange={setFreezing} />
      <AdminDeleteDialog
        open={deleting}
        onOpenChange={setDeleting}
        title={`Delete ${org.name}?`}
        noun="organization"
        slug={org.slug}
        consequences={<OrgDeleteConsequences org={org} />}
        run={(confirm) => del.mutateAsync({ orgId: org.id, confirm })}
        onDone={() => onDeleted(`Deleted the organization ${org.name} (${org.slug}).`)}
      />
    </>
  );
}

/** Instance administration, Organizations (RFE-006, P5-ADMDEL): counts and state, never content. */
export function AdminOrgsView() {
  const [frozenOnly, setFrozenOnly] = useState(false);
  const [done, setDone] = useState<string | null>(null);
  const [failed, setFailed] = useState<string | null>(null);
  const q = useAdminOrgs(frozenOnly);
  if (q.isPending) return <Loading />;
  if (q.isError) return <ErrorState error={q.error} onRetry={() => q.refetch()} />;
  return (
    <div className="max-w-page space-y-6">
      <PageHeader
        title="Organizations"
        description="Freeze or delete an organization as the operator, for an incident such as a compromised account. You see counts and names, never the organization's data."
      />
      {done ? (
        <Notice tone="accent" role="status" data-testid="admin-orgs-done">
          {done}
        </Notice>
      ) : null}
      {failed ? (
        <Notice tone="danger" role="alert">
          {failed}
        </Notice>
      ) : null}
      <CheckboxField id="orgs-frozen-only" label="Frozen organizations only" checked={frozenOnly} onChange={setFrozenOnly} />
      {q.data.truncated ? <MoreNotShown what="organizations" narrow="Show frozen organizations only to narrow the list." /> : null}
      {q.data.items.length === 0 ? (
        <p className="text-sm text-ink-2">{frozenOnly ? "No organization is frozen." : "No organizations."}</p>
      ) : (
        <DataTable caption="Organizations">
          <thead>
            <tr>
              <Th>Name</Th>
              <Th>Slug</Th>
              <Th className="text-right">Members</Th>
              <Th className="text-right">Workspaces</Th>
              <Th className="text-right">Agents</Th>
              <Th>Created</Th>
              <Th>State</Th>
              <Th className="text-right">
                <span className="sr-only">Actions</span>
              </Th>
            </tr>
          </thead>
          <tbody>
            {q.data.items.map((o) => (
              <tr key={o.id} id={`org-${o.slug}`} className="scroll-mt-24 target:bg-accent-soft">
                <Td className="font-medium">
                  <Link href={adminRoutes.organization(o.id)} className="underline-offset-2 hover:underline">
                    {o.name}
                  </Link>
                </Td>
                <Td className="font-mono text-xs text-muted">{o.slug}</Td>
                <Td className="text-right tabular-nums">{o.member_count}</Td>
                <Td className="text-right tabular-nums">{o.workspace_count}</Td>
                <Td className="text-right tabular-nums">{o.agent_count}</Td>
                <Td className="whitespace-nowrap text-ink-2">{fmtDate(o.created_at)}</Td>
                <Td>
                  <div className="flex flex-wrap gap-1">
                    {o.frozen ? <Badge tone="danger">Frozen</Badge> : null}
                    {o.ownerless ? <Badge>No owner</Badge> : null}
                  </div>
                </Td>
                <Td className="text-right">
                  <OrgRowActions
                    org={o}
                    onDeleted={(m) => {
                      setFailed(null);
                      setDone(m);
                    }}
                    onError={(m) => {
                      setDone(null);
                      setFailed(m);
                    }}
                  />
                </Td>
              </tr>
            ))}
          </tbody>
        </DataTable>
      )}
    </div>
  );
}
