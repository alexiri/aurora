"use client";

import Link from "next/link";
import { useState } from "react";

import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { DataTable, Td, Th } from "@/components/ui/data-table";
import { inputClass } from "@/components/ui/field";
import { PageHeader } from "@/components/ui/page-header";
import { Section } from "@/components/ui/section";
import { sourceLabel, useGroups, type Group } from "@/lib/api/groups";

import { CreateGroupDialog } from "./create-group-dialog";

export function OrgRoleChip({ role }: { role: Group["org_role"] }) {
  if (!role) return <span className="text-muted">—</span>;
  return <Badge tone={role === "admin" ? "pending" : "neutral"}>{role}</Badge>;
}

export function GroupsView({ orgId, orgSlug }: { orgId: string; orgSlug: string }) {
  const [q, setQ] = useState("");
  const groups = useGroups(orgId, q.trim());
  const items = groups.data?.items ?? [];
  return (
    <div className="max-w-[1160px] space-y-8">
      <PageHeader
        title="Groups"
        description="Named sets of people that can hold workspace access and an organization role."
        actions={<CreateGroupDialog orgId={orgId} orgSlug={orgSlug} />}
        className="mb-6"
      />
      <Section id="groups" title="All groups">
        <div className="space-y-3">
          <input
            type="search"
            aria-label="Search groups"
            placeholder="Search groups"
            className={`${inputClass} max-w-sm`}
            value={q}
            onChange={(e) => setQ(e.target.value)}
          />
          {groups.isPending ? <Loading /> : null}
          {groups.isError ? <ErrorState error={groups.error} onRetry={() => groups.refetch()} /> : null}
          {groups.data && items.length === 0 ? (
            <EmptyState title={q ? "No group matches" : "No groups yet"}>
              {q ? null : "Create a group to give a team access to workspaces in one place."}
            </EmptyState>
          ) : null}
          {items.length > 0 ? (
            <DataTable caption="Groups">
              <thead>
                <tr>
                  <Th>Name</Th>
                  <Th>Members from</Th>
                  <Th>Members</Th>
                  <Th>Org role</Th>
                  <Th>Workspaces</Th>
                </tr>
              </thead>
              <tbody>
                {items.map((g) => (
                  <tr key={g.id}>
                    <Td>
                      <Link
                        href={`/o/${orgSlug}/groups/${g.id}`}
                        className="font-medium text-accent-ink hover:underline"
                      >
                        {g.name}
                      </Link>
                      {g.description ? <div className="text-xs text-muted">{g.description}</div> : null}
                    </Td>
                    <Td>
                      <Badge tone={g.source === "manual" ? "neutral" : "accent"}>{sourceLabel(g.source)}</Badge>
                    </Td>
                    <Td>{g.member_count}</Td>
                    <Td>
                      <OrgRoleChip role={g.org_role} />
                    </Td>
                    <Td>{g.workspace_count}</Td>
                  </tr>
                ))}
              </tbody>
            </DataTable>
          ) : null}
        </div>
      </Section>
    </div>
  );
}
