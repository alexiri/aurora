"use client";

import { useRouter } from "next/navigation";
import { useState, type FormEvent } from "react";

import { fmtDate } from "@/components/account/key-table";
import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ConfirmDialog } from "@/components/ui/confirm-dialog";
import { DataTable, Td, Th } from "@/components/ui/data-table";
import { Field, inputClass } from "@/components/ui/field";
import { Notice } from "@/components/ui/notice";
import { matchesFilter, PickerFilter } from "@/components/ui/picker-filter";
import { PageHeader } from "@/components/ui/page-header";
import { Section } from "@/components/ui/section";
import { cn } from "@/lib/utils";
import { useOrgMembers } from "@/lib/api/account";
import { errorMessage } from "@/lib/api/errors";
import {
  isManaged,
  isStaleGroup,
  sourceLabel,
  useChangeMembers,
  useDeleteGroup,
  useDeleteGroupAccess,
  useGroup,
  useGroupImpact,
  useGroupMembers,
  useGroupWorkspaces,
  usePatchGroup,
  usePutGroupAccess,
  type Group,
  type WorkspaceRole,
} from "@/lib/api/groups";
import { useWorkspaces } from "@/lib/api/queries";

import { ImpactSummary } from "./impact-summary";

const ROLE_HELP = "Owner manages the workspace; Approver also decides approvals; Member chats with agents.";

function Details({ g, orgId }: { g: Group; orgId: string }) {
  const patch = usePatchGroup(g.id, orgId);
  const [name, setName] = useState(g.name);
  const [description, setDescription] = useState(g.description ?? "");
  const scim = g.source === "scim";
  function submit(e: FormEvent) {
    e.preventDefault();
    patch.mutate({
      name: scim ? undefined : name.trim(),
      description: description.trim() || null,
      revision: g.revision,
    });
  }
  return (
    <Section id="grp-details" title="Details" card>
      <form onSubmit={submit} aria-label="Group details" className="flex flex-wrap items-start gap-x-4 gap-y-3">
        <Field id="gd-name" label="Name">
          <input
            id="gd-name"
            className={cn(inputClass, "w-72 max-w-full")}
            value={name}
            maxLength={80}
            disabled={scim}
            onChange={(e) => setName(e.target.value)}
          />
        </Field>
        <Field id="gd-desc" label="Description">
          <input
            id="gd-desc"
            className={cn(inputClass, "w-72 max-w-full sm:w-96")}
            value={description}
            maxLength={280}
            onChange={(e) => setDescription(e.target.value)}
          />
        </Field>
        <Button type="submit" disabled={patch.isPending} className="sm:mt-label-offset">
          Save
        </Button>
        {patch.error ? (
          <Notice tone="danger" role="alert" className="basis-full">
            {isStaleGroup(patch.error)
              ? "This group was changed by someone else. The latest version has been loaded; review your edit and save again."
              : errorMessage(patch.error)}
          </Notice>
        ) : null}
      </form>
    </Section>
  );
}

function RemoveMemberDialog({
  g,
  orgId,
  target,
  onClose,
}: {
  g: Group;
  orgId: string;
  target: { id: string; label: string } | null;
  onClose: () => void;
}) {
  const change = useChangeMembers(g.id, orgId);
  const impact = useGroupImpact(g.id, target?.id ?? null, target !== null);
  return (
    <ConfirmDialog
      open={target !== null}
      onOpenChange={(o) => {
        if (!o) {
          onClose();
          change.reset();
        }
      }}
      title="Remove member?"
      description={`${target?.label ?? ""} will lose the access this group gives.`}
      confirmLabel="Remove member"
      danger
      busy={change.isPending}
      onConfirm={() => target && change.mutate({ remove: [target.id] }, { onSuccess: onClose })}
    >
      <div className="space-y-3">
        <ImpactSummary query={impact} />
        {change.error ? (
          <Notice tone="danger" role="alert">
            {errorMessage(change.error)}
          </Notice>
        ) : null}
      </div>
    </ConfirmDialog>
  );
}

function Members({ g, orgId }: { g: Group; orgId: string }) {
  const members = useGroupMembers(g.id);
  const orgMembers = useOrgMembers(orgId);
  const change = useChangeMembers(g.id, orgId);
  const [pick, setPick] = useState("");
  const [filter, setFilter] = useState("");
  const [target, setTarget] = useState<{ id: string; label: string } | null>(null);
  const managed = isManaged(g);
  if (members.isPending) return <Loading />;
  if (members.isError) return <ErrorState error={members.error} onRetry={() => members.refetch()} />;
  const items = members.data.pages.flatMap((p) => p.items);
  const inGroup = new Set(items.map((m) => m.user.id));
  const eligible = (orgMembers.data?.items ?? []).filter((m) => !inGroup.has(m.user_id));
  const candidates = eligible.filter((m) => m.user_id === pick || matchesFilter(filter, m.display_name, m.email));
  return (
    <Section
      id="grp-members"
      title={`Members (${g.member_count})`}
      description={managed ? undefined : "At most 5 000 members per group."}
    >
      <div className="space-y-3">
        {g.source === "idp" ? (
          <Notice role="status">
            Members come from your sign-in provider (claim value <code>{g.idp_value}</code>). They are updated
            each time someone signs in.
          </Notice>
        ) : null}
        {g.source === "scim" ? <Notice role="status">Managed by your identity provider.</Notice> : null}
        {!managed ? (
          <form
            aria-label="Add member"
            className="flex flex-wrap items-end gap-3"
            onSubmit={(e) => {
              e.preventDefault();
              if (pick) change.mutate({ add: [pick] }, { onSuccess: () => setPick("") });
            }}
          >
            <PickerFilter id="gm-filter" label="Find by name or e-mail" count={eligible.length} value={filter} onChange={setFilter} />
            <Field id="gm-add" label="Add a member">
              <select
                id="gm-add"
                className={cn(inputClass, "w-72 max-w-full")}
                value={pick}
                onChange={(e) => setPick(e.target.value)}
              >
                <option value="">Select a person…</option>
                {candidates.map((m) => (
                  <option key={m.user_id} value={m.user_id}>
                    {m.display_name} ({m.email})
                  </option>
                ))}
              </select>
            </Field>
            <Button type="submit" disabled={!pick || change.isPending}>
              Add
            </Button>
          </form>
        ) : null}
        {change.error ? (
          <Notice tone="danger" role="alert">
            {errorMessage(change.error)}
          </Notice>
        ) : null}
        {items.length === 0 ? (
          <EmptyState title="No members" />
        ) : (
          <DataTable caption="Group members">
            <thead>
              <tr>
                <Th>Person</Th>
                <Th>Status</Th>
                {g.source === "idp" ? <Th>Last sync</Th> : null}
                <Th>
                  <span className="sr-only">Actions</span>
                </Th>
              </tr>
            </thead>
            <tbody>
              {items.map((m) => (
                <tr key={m.user.id}>
                  <Td>
                    <div className="font-medium">{m.user.display_name}</div>
                    <div className="text-xs text-muted">{m.user.email}</div>
                  </Td>
                  <Td>
                    <Badge tone={m.status === "active" ? "accent" : "pending"}>{m.status}</Badge>
                    {m.status === "suspended" ? (
                      <div className="text-xs text-muted">Suspended members derive no access.</div>
                    ) : null}
                  </Td>
                  {g.source === "idp" ? <Td className="whitespace-nowrap text-ink-2">{fmtDate(m.synced_at)}</Td> : null}
                  <Td className="text-right">
                    {managed ? null : (
                      <Button
                        variant="outline"
                        size="sm"
                        aria-label={`Remove ${m.user.email}`}
                        onClick={() => setTarget({ id: m.user.id, label: m.user.display_name || m.user.email })}
                      >
                        Remove
                      </Button>
                    )}
                  </Td>
                </tr>
              ))}
            </tbody>
          </DataTable>
        )}
        {members.hasNextPage ? (
          <Button variant="outline" disabled={members.isFetchingNextPage} onClick={() => void members.fetchNextPage()}>
            Load more
          </Button>
        ) : null}
        <RemoveMemberDialog g={g} orgId={orgId} target={target} onClose={() => setTarget(null)} />
      </div>
    </Section>
  );
}

function OrgRole({ g, orgId }: { g: Group; orgId: string }) {
  const patch = usePatchGroup(g.id, orgId);
  const [pending, setPending] = useState<"admin" | null>(null);
  const apply = (v: "member" | "admin" | null) => patch.mutate({ org_role: v, revision: g.revision });
  return (
    <Section id="grp-role" title="Organization role" card>
      <div className="space-y-3">
        <Field id="gr-role" label="Members act as">
          <select
            id="gr-role"
            className={cn(inputClass, "w-48")}
            value={g.org_role ?? ""}
            disabled={patch.isPending}
            onChange={(e) => {
              const v = e.target.value;
              if (v === "admin") setPending("admin");
              else apply(v === "member" ? "member" : null);
            }}
          >
            <option value="">None</option>
            <option value="member">Member</option>
            <option value="admin">Admin</option>
          </select>
        </Field>
        <p className="text-sm text-ink-2">
          {g.org_role
            ? `Members of this group act as ${g.org_role} in the whole organization, including every workspace.`
            : "No organization role. Owner can never be given to a group."}
        </p>
        {patch.error ? (
          <Notice tone="danger" role="alert">
            {errorMessage(patch.error)}
          </Notice>
        ) : null}
        <ConfirmDialog
          open={pending !== null}
          onOpenChange={(o) => !o && setPending(null)}
          title="Make this group Admin?"
          description="Every member of this group will own every workspace in the organization and can manage groups."
          confirmLabel="Make Admin"
          danger
          busy={patch.isPending}
          onConfirm={() => {
            apply("admin");
            setPending(null);
          }}
        />
      </div>
    </Section>
  );
}

function Workspaces({ g, orgId }: { g: Group; orgId: string }) {
  const bindings = useGroupWorkspaces(g.id);
  const all = useWorkspaces(orgId);
  const [ws, setWs] = useState("");
  const [filter, setFilter] = useState("");
  const [role, setRole] = useState<WorkspaceRole>("member");
  const [err, setErr] = useState<unknown>(null);
  // The mutation hooks are bound to a workspace: use the picked one for adds, each row's for edits.
  const put = usePutGroupAccess(ws);
  if (bindings.isPending) return <Loading />;
  if (bindings.isError) return <ErrorState error={bindings.error} onRetry={() => bindings.refetch()} />;
  const items = bindings.data.items;
  const bound = new Set(items.map((b) => b.workspace.id));
  const unbound = (all.data?.items ?? []).filter((w) => !bound.has(w.id));
  const candidates = unbound.filter((w) => w.id === ws || matchesFilter(filter, w.name, w.slug));
  return (
    <Section id="grp-ws" title="Workspace access" description={ROLE_HELP}>
      <div className="space-y-3">
        {err ? (
          <Notice tone="danger" role="alert">
            {errorMessage(err)}
          </Notice>
        ) : null}
        {items.length === 0 ? (
          <EmptyState title="Not bound to any workspace" />
        ) : (
          <DataTable caption="Workspaces this group can use">
            <thead>
              <tr>
                <Th>Workspace</Th>
                <Th>Role</Th>
                <Th>
                  <span className="sr-only">Actions</span>
                </Th>
              </tr>
            </thead>
            <tbody>
              {items.map((b) => (
                <BindingRow key={b.workspace.id} g={g} b={b} onError={setErr} />
              ))}
            </tbody>
          </DataTable>
        )}
        <form
          aria-label="Add workspace"
          className="flex flex-wrap items-end gap-3"
          onSubmit={(e) => {
            e.preventDefault();
            if (!ws) return;
            setErr(null);
            put.mutate({ group: g.id, role }, { onSuccess: () => setWs(""), onError: setErr });
          }}
        >
          <PickerFilter id="gw-filter" label="Find a workspace" count={unbound.length} value={filter} onChange={setFilter} />
          <Field id="gw-ws" label="Workspace">
            <select id="gw-ws" className={cn(inputClass, "w-64 max-w-full")} value={ws} onChange={(e) => setWs(e.target.value)}>
              <option value="">Select a workspace…</option>
              {candidates.map((w) => (
                <option key={w.id} value={w.id}>
                  {w.name}
                </option>
              ))}
            </select>
          </Field>
          <Field id="gw-role" label="Role">
            <select
              id="gw-role"
              className={cn(inputClass, "w-40")}
              value={role}
              onChange={(e) => setRole(e.target.value as WorkspaceRole)}
            >
              <option value="member">member</option>
              <option value="approver">approver</option>
              <option value="owner">owner</option>
            </select>
          </Field>
          <Button type="submit" disabled={!ws || put.isPending}>
            Add
          </Button>
        </form>
      </div>
    </Section>
  );
}

function BindingRow({
  g,
  b,
  onError,
}: {
  g: Group;
  b: { workspace: { id: string; name: string }; role: WorkspaceRole };
  onError: (e: unknown) => void;
}) {
  const put = usePutGroupAccess(b.workspace.id);
  const del = useDeleteGroupAccess(b.workspace.id);
  const [confirm, setConfirm] = useState(false);
  // Unbinding takes away this workspace only, not the whole group (C-464 `unbind_workspace`).
  const impact = useGroupImpact(g.id, null, confirm, b.workspace.id);
  return (
    <tr>
      <Td className="font-medium">{b.workspace.name}</Td>
      <Td>
        <select
          aria-label={`Role in ${b.workspace.name}`}
          className={cn(inputClass, "w-36")}
          value={b.role}
          disabled={put.isPending}
          onChange={(e) => put.mutate({ group: g.id, role: e.target.value as WorkspaceRole }, { onError })}
        >
          <option value="member">member</option>
          <option value="approver">approver</option>
          <option value="owner">owner</option>
        </select>
      </Td>
      <Td className="text-right">
        <Button variant="outline" size="sm" aria-label={`Remove access to ${b.workspace.name}`} onClick={() => setConfirm(true)}>
          Remove
        </Button>
        <ConfirmDialog
          open={confirm}
          onOpenChange={setConfirm}
          title="Remove workspace access?"
          description={`Members of ${g.name} will no longer get access to ${b.workspace.name} through this group.`}
          confirmLabel="Remove access"
          danger
          busy={del.isPending}
          onConfirm={() => del.mutate(g.id, { onSuccess: () => setConfirm(false), onError })}
        >
          <ImpactSummary query={impact} />
        </ConfirmDialog>
      </Td>
    </tr>
  );
}

function DeleteGroup({ g, orgId, orgSlug }: { g: Group; orgId: string; orgSlug: string }) {
  const router = useRouter();
  const del = useDeleteGroup(g.id, orgId);
  const [open, setOpen] = useState(false);
  const scim = g.source === "scim";
  const impact = useGroupImpact(g.id, null, open);
  return (
    <Section id="grp-danger" title="Delete group" card>
      <div className="space-y-3">
        <p className="text-sm text-ink-2">
          Deleting a group removes its members and bindings. Policy rules that name it stay as they are and match
          nobody.
        </p>
        {scim ? (
          <p className="text-sm text-muted">
            This group is managed by your directory (SCIM): delete it in your identity provider.
          </p>
        ) : null}
        <Button variant="danger" disabled={scim} onClick={() => setOpen(true)}>
          Delete group
        </Button>
        <ConfirmDialog
          open={open}
          onOpenChange={setOpen}
          title={`Delete ${g.name}?`}
          confirmLabel="Delete group"
          danger
          busy={del.isPending}
          onConfirm={() => del.mutate(undefined, { onSuccess: () => router.push(`/o/${orgSlug}/groups`) })}
        >
          <div className="space-y-3">
            <ImpactSummary query={impact} />
            {del.error ? (
              <Notice tone="danger" role="alert">
                {errorMessage(del.error)}
              </Notice>
            ) : null}
          </div>
        </ConfirmDialog>
      </div>
    </Section>
  );
}

export function GroupDetail({ orgId, orgSlug, id }: { orgId: string; orgSlug: string; id: string }) {
  const group = useGroup(id);
  if (group.isPending) return <Loading />;
  if (group.isError) return <ErrorState error={group.error} onRetry={() => group.refetch()} />;
  const g = group.data;
  return (
    <div className="max-w-page space-y-8">
      <PageHeader
        breadcrumb={[{ label: "Groups", href: `/o/${orgSlug}/groups` }, { label: g.name }]}
        title={
          <span className="flex flex-wrap items-center gap-3">
            {g.name}
            <Badge tone={g.source === "manual" ? "neutral" : "accent"}>{sourceLabel(g.source)}</Badge>
          </span>
        }
        description={g.description ?? undefined}
        className="mb-6"
      />
      <Details g={g} orgId={orgId} />
      <Members g={g} orgId={orgId} />
      <OrgRole g={g} orgId={orgId} />
      <Workspaces g={g} orgId={orgId} />
      <DeleteGroup g={g} orgId={orgId} orgSlug={orgSlug} />
    </div>
  );
}
