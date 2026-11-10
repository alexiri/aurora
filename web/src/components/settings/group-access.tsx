"use client";

import { useState } from "react";

import { ImpactSummary } from "@/components/groups/impact-summary";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ConfirmDialog } from "@/components/ui/confirm-dialog";
import { DataTable, Td, Th } from "@/components/ui/data-table";
import { Field, inputClass } from "@/components/ui/field";
import { Notice } from "@/components/ui/notice";
import { Section } from "@/components/ui/section";
import { cn } from "@/lib/utils";
import type { components } from "@/lib/api/schema";
import { errorMessage } from "@/lib/api/errors";
import {
  sourceText,
  useDeleteGroupAccess,
  useGroupImpact,
  useGroups,
  usePutGroupAccess,
  type DerivedMember,
  type GroupAccessBinding,
  type WorkspaceRole,
} from "@/lib/api/groups";

type AccessList = components["schemas"]["AccessList"];

function BoundGroup({ wsId, b, canOwn, onError }: { wsId: string; b: GroupAccessBinding; canOwn: boolean; onError: (e: unknown) => void }) {
  const put = usePutGroupAccess(wsId);
  const del = useDeleteGroupAccess(wsId);
  const [confirm, setConfirm] = useState(false);
  const impact = useGroupImpact(b.group.id, null, confirm);
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
          <select
            aria-label={`Role for group ${b.group.name}`}
            className={cn(inputClass, "w-36")}
            value={b.role}
            disabled={put.isPending}
            onChange={(e) => put.mutate({ group: b.group.id, role: e.target.value as WorkspaceRole }, { onError })}
          >
            <option value="member">member</option>
            <option value="approver">approver</option>
            <option value="owner">owner</option>
          </select>
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
          <select
            id="acc-group-role"
            className={cn(inputClass, "w-40 max-w-full")}
            value={role}
            onChange={(e) => setRole(e.target.value as WorkspaceRole)}
          >
            <option value="member">member</option>
            <option value="approver">approver</option>
            <option value="owner">owner</option>
          </select>
        </Field>
        <Button type="submit" disabled={!group || put.isPending} className="sm:mt-[1.625rem]">
          Add
        </Button>
        {put.error ? (
          <Notice tone="danger" role="alert" className="basis-full">
            {errorMessage(put.error)}
          </Notice>
        ) : null}
      </form>
    </Section>
  );
}

function Derived({ rows }: { rows: DerivedMember[] }) {
  if (rows.length === 0) return null;
  return (
    <Section
      id="acc-derived"
      title="Members with access"
      description="Everyone who has access through a group or an organization role, and why."
    >
      <DataTable caption="Members with access through groups or organization roles">
        <thead>
          <tr>
            <Th>Person</Th>
            <Th>Effective role</Th>
            <Th>Why</Th>
          </tr>
        </thead>
        <tbody>
          {rows.map((d) => (
            <tr key={d.user.id}>
              <Td>
                <div className="font-medium">{d.user.display_name}</div>
                <div className="text-xs text-muted">{d.user.email}</div>
              </Td>
              <Td>{d.role}</Td>
              <Td>
                <span className="flex flex-wrap gap-1.5">
                  {d.sources.map((s, i) => (
                    <Badge key={i} className="normal-case tracking-normal">
                      {sourceText(s)}
                    </Badge>
                  ))}
                </span>
              </Td>
            </tr>
          ))}
        </tbody>
      </DataTable>
    </Section>
  );
}

/** The Groups part of the workspace Access section (`04 §Groups`; `can_own`). */
export function GroupAccess({ wsId, orgId, canOwn, data }: { wsId: string; orgId: string; canOwn: boolean; data: AccessList }) {
  const [err, setErr] = useState<unknown>(null);
  const bindings = data.group_bindings ?? [];
  const derived = data.derived_members?.items ?? [];
  const bound = new Set(bindings.map((b) => b.group.id));
  return (
    <div className="space-y-6">
      <Section id="acc-groups" title="Groups" description="Groups bound to this workspace and the role each gives its members.">
        <div className="space-y-3">
          {err ? (
            <Notice tone="danger" role="alert">
              {errorMessage(err)}
            </Notice>
          ) : null}
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
      <Derived rows={derived} />
    </div>
  );
}
