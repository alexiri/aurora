"use client";

import { useState } from "react";

import { ImpactSummary } from "@/components/groups/impact-summary";
import { Button } from "@/components/ui/button";
import { ConfirmDialog } from "@/components/ui/confirm-dialog";
import { DataTable, Td, Th } from "@/components/ui/data-table";
import { Field, inputClass } from "@/components/ui/field";
import { ErrorNotice } from "@/components/ui/notice";
import { Section } from "@/components/ui/section";
import { Select } from "@/components/ui/select";
import { cn } from "@/lib/utils";
import type { components } from "@/lib/api/schema";
import {
  useDeleteGroupAccess,
  useGroupImpact,
  useGroups,
  usePutGroupAccess,
  type GroupAccessBinding,
  type WorkspaceRole,
} from "@/lib/api/groups";

type AccessList = components["schemas"]["AccessList"];

function BoundGroup({ wsId, b, canOwn, onError }: { wsId: string; b: GroupAccessBinding; canOwn: boolean; onError: (e: unknown) => void }) {
  const put = usePutGroupAccess(wsId);
  const del = useDeleteGroupAccess(wsId);
  const [confirm, setConfirm] = useState(false);
  // Unbinding takes away this workspace only, not the whole group (C-464 `unbind_workspace`).
  const impact = useGroupImpact(b.group.id, null, confirm, wsId);
  return (
    <tr>
      <Td>
        <div className="font-medium">{b.group.name}</div>
        <div className="text-xs text-muted">
          {b.group.member_count} {b.group.member_count === 1 ? "member" : "members"}
          {b.group.source === "manual" ? "" : ` · from ${b.group.source === "idp" ? "sign-in" : "SCIM"}`}
        </div>
      </Td>
      <Td>
        {canOwn ? (
          <Select aria-label={`Role for group ${b.group.name}`} value={b.role} disabled={put.isPending} className="w-36" onChange={(v) => put.mutate({ group: b.group.id, role: v as WorkspaceRole }, { onError })} options={[{ value: "member", label: "member" }, { value: "approver", label: "approver" }, { value: "owner", label: "owner" }]} />
        ) : (
          b.role
        )}
      </Td>
      <Td className="text-right">
        {canOwn ? (
          <>
            <Button variant="outline" size="sm" aria-label={`Remove group ${b.group.name}`} onClick={() => setConfirm(true)}>
              Remove
            </Button>
            <ConfirmDialog
              open={confirm}
              onOpenChange={setConfirm}
              title="Remove group access?"
              description={`Members of ${b.group.name} will lose the access this group gives to the workspace.`}
              confirmLabel="Remove group"
              danger
              busy={del.isPending}
              onConfirm={() => del.mutate(b.group.id, { onSuccess: () => setConfirm(false), onError })}
            >
              <ImpactSummary query={impact} />
            </ConfirmDialog>
          </>
        ) : null}
      </Td>
    </tr>
  );
}

function AddGroup({ wsId, orgId, bound }: { wsId: string; orgId: string; bound: Set<string> }) {
  const groups = useGroups(orgId);
  const put = usePutGroupAccess(wsId);
  const [group, setGroup] = useState("");
  const [role, setRole] = useState<WorkspaceRole>("member");
  if (groups.isPending || groups.isError) return null;
  const candidates = groups.data.items.filter((g) => !bound.has(g.id));
  if (candidates.length === 0) return null;
  return (
    <Section id="acc-add-group" title="Add group" description="Everyone in the group gets this role here." card>
      <form
        aria-label="Add group"
        className="flex flex-wrap items-start gap-x-4 gap-y-3"
        onSubmit={(e) => {
          e.preventDefault();
          if (group) put.mutate({ group, role }, { onSuccess: () => setGroup("") });
        }}
      >
        <Field id="acc-group" label="Group">
          <select id="acc-group" className={cn(inputClass, "w-72 max-w-full")} value={group} onChange={(e) => setGroup(e.target.value)}>
            <option value="">Select a group…</option>
            {candidates.map((g) => (
              <option key={g.id} value={g.id}>
                {g.name} ({g.member_count})
              </option>
            ))}
          </select>
        </Field>
        <Field id="acc-group-role" label="Role">
          <Select id="acc-group-role" value={role} className="w-40 max-w-full" onChange={(v) => setRole(v as WorkspaceRole)} options={[{ value: "member", label: "member" }, { value: "approver", label: "approver" }, { value: "owner", label: "owner" }]} />
        </Field>
        <Button type="submit" disabled={!group || put.isPending} className="sm:mt-label-offset">
          Add
        </Button>
        <ErrorNotice error={put.error} className="basis-full" />
      </form>
    </Section>
  );
}

/** The Groups part of the workspace Access section (`04 §Groups`; `can_own`). */
export function GroupAccess({ wsId, orgId, canOwn, data }: { wsId: string; orgId: string; canOwn: boolean; data: AccessList }) {
  const [err, setErr] = useState<unknown>(null);
  const bindings = data.group_bindings ?? [];
  const bound = new Set(bindings.map((b) => b.group.id));
  return (
    <div className="space-y-6">
      <Section id="acc-groups" title="Groups" description="Groups bound to this workspace and the role each gives its members.">
        <div className="space-y-3">
          <ErrorNotice error={err} />
          {bindings.length === 0 ? (
            <p className="text-sm text-ink-2">No group is bound to this workspace.</p>
          ) : (
            <DataTable caption="Groups bound to this workspace">
              <thead>
                <tr>
                  <Th>Group</Th>
                  <Th>Role</Th>
                  <Th>
                    <span className="sr-only">Actions</span>
                  </Th>
                </tr>
              </thead>
              <tbody>
                {bindings.map((b) => (
                  <BoundGroup key={b.group.id} wsId={wsId} b={b} canOwn={canOwn} onError={setErr} />
                ))}
              </tbody>
            </DataTable>
          )}
        </div>
      </Section>
      {canOwn ? <AddGroup wsId={wsId} orgId={orgId} bound={bound} /> : null}
      {/* (P5-POLISH-7) The people who have access through a group are rows of the People table, with their reason. */}
    </div>
  );
}
