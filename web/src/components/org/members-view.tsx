"use client";

import { useState } from "react";

import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { OffboardingDialog } from "@/components/automations/offboarding-dialog";
import { IdpRoleBadge } from "@/components/identity/idp-role-badge";
import { DataTable, Td, Th } from "@/components/ui/data-table";
import { inputClass } from "@/components/ui/field";
import { LoadMore } from "@/components/ui/load-more";
import { ErrorNotice, Notice } from "@/components/ui/notice";
import { PageHeader } from "@/components/ui/page-header";
import { Section } from "@/components/ui/section";
import { cn } from "@/lib/utils";
import {
  canManageMember,
  roleOptions,
  useAllMembers,
  useCancelInvitation,
  useInvitations,
  usePatchMember,
  useRemoveMember,
  type OrgMember,
  type OrgRole,
} from "@/lib/api/org";
import { useOrgOffboarding } from "@/lib/api/automations";
import { When } from "@/components/ui/when";
import { ConfirmDialog } from "@/components/ui/confirm-dialog";
import { useInstance, useMe, useOrg } from "@/lib/api/queries";
import { Menu, MenuContent, MenuItem, MenuTrigger } from "@/components/ui/menu";

import { InviteDialog } from "./invite-dialog";

const cap = (s: string) => `${s.charAt(0).toUpperCase()}${s.slice(1)}`;

const ROLE_RANK: Record<OrgRole, number> = { member: 0, admin: 1, owner: 2 };
const PROMOTE_SENTENCE: Partial<Record<OrgRole, string>> = {
  owner: "Owners can change settings, add and remove people and delete the organization.",
  admin: "Admins can change settings and add and remove people. They can't delete the organization.",
};

export function MemberRow({
  m,
  viewer,
  busy,
  onRole,
  onStatus,
  onRemove,
  mine = false,
  lastOwner = false,
  orgName,
}: {
  m: OrgMember;
  viewer: OrgRole;
  busy?: boolean;
  onRole: (role: OrgRole) => void;
  onStatus: (status: "active" | "suspended") => void;
  onRemove: () => void;
  /** (P5-POLISH-9, D6) The viewer's own row: no menu, no role change, no removal. */
  mine?: boolean;
  /** The only owner: the roles below Owner are not offered. */
  lastOwner?: boolean;
  orgName?: string;
}) {
  const fromIdp = m.idp_value ?? null;
  // A role the IdP gives is read-only here (409 managed_by_idp on the API); nobody changes their own, and the last owner stays owner.
  const options = fromIdp || mine ? [] : lastOwner ? roleOptions(viewer, m.role).filter((r) => r === "owner") : roleOptions(viewer, m.role);
  const manage = canManageMember(viewer, m.role) && !mine;
  return (
    <tr>
      <Td cell="title">
        <div className="font-medium">
          {m.display_name}
          {mine ? <span className="ml-1 font-normal text-muted">(you)</span> : null}
        </div>
        <div className="break-all text-muted">{m.email}</div>
      </Td>
      <Td>
        {options.length > 0 ? (
          <>
            <select
              aria-label={`Role for ${m.email}`}
              aria-describedby={m.directory ? `directory-role-${m.user_id}` : undefined}
              className={cn(inputClass, "min-w-28")}
              value={m.role}
              disabled={busy || !!m.directory}
              onChange={(e) => onRole(e.target.value as OrgRole)}
            >
              {options.map((r) => (
                <option key={r} value={r}>
                  {cap(r)}
                </option>
              ))}
            </select>
            {m.directory ? (
              <p id={`directory-role-${m.user_id}`} className="mt-1 text-xs text-muted">
                Managed by your identity provider: the role comes from directory sync.
              </p>
            ) : null}
          </>
        ) : (
          <span className="flex flex-wrap items-center gap-1.5">
            {cap(m.role)}
            {fromIdp ? <IdpRoleBadge value={fromIdp} /> : null}
          </span>
        )}
      </Td>
      <Td>
        {m.status === "pending" ? (
          <Badge
            title="Your identity provider prepared this membership for an existing account. It becomes active at the person's next sign-in."
            className="normal-case tracking-normal"
          >
            Waiting for first sign-in
          </Badge>
        ) : m.status === "active" ? null : (
          // Only when not active: an active member needs no chip.
          <Badge>{cap(m.status)}</Badge>
        )}
        {m.directory ? (
          <Badge
            tone="neutral"
            title="Managed by directory sync: this member is provisioned by your identity provider (SCIM)."
            className="ml-1.5 normal-case tracking-normal"
          >
            Directory{m.directory.state === "suspended" ? " (suspended)" : ""}
          </Badge>
        ) : null}
      </Td>
      <Td label="Joined" className="whitespace-nowrap text-ink-2">
        <When value={m.created_at} />
      </Td>
      <Td cell="actions" className="text-right">
        {manage && !(m.status === "pending" && fromIdp) ? (
          <Menu>
            <MenuTrigger asChild>
              <Button variant="outline" size="sm" disabled={busy} aria-label={`More for ${m.display_name || m.email}`}>
                <span aria-hidden="true">…</span>
              </Button>
            </MenuTrigger>
            <MenuContent align="end">
              {m.status === "pending" ? null : (
                <MenuItem
                  aria-label={`${m.status === "active" ? "Suspend" : "Reactivate"} ${m.email}`}
                  onSelect={() => onStatus(m.status === "active" ? "suspended" : "active")}
                >
                  <span>
                    {m.status === "active" ? "Suspend" : "Reactivate"}
                    {m.status === "active" ? <span className="block text-xs text-muted">Can&apos;t sign in until you resume them</span> : null}
                  </span>
                </MenuItem>
              )}
              {fromIdp ? null : (
                <MenuItem className="text-danger-ink" aria-label={`Remove ${m.email}`} onSelect={onRemove}>
                  Remove from {orgName ?? "the organization"}…
                </MenuItem>
              )}
            </MenuContent>
          </Menu>
        ) : null}
      </Td>
    </tr>
  );
}

function OffboardDialog({
  orgId,
  target,
  members,
  busy,
  error,
  onConfirm,
  onClose,
}: {
  orgId: string;
  target: { m: OrgMember; mode: "remove" | "suspend" } | null;
  members: OrgMember[];
  busy: boolean;
  error: unknown;
  onConfirm: (reassignTo: string | undefined) => void;
  onClose: () => void;
}) {
  const q = useOrgOffboarding(orgId, target?.m.user_id ?? null);
  const suspend = target?.mode === "suspend";
  const email = target?.m.email ?? "";
  return (
    <OffboardingDialog
      open={target !== null}
      title={suspend ? "Suspend member?" : "Remove member?"}
      description={
        suspend
          ? `${email} will be signed out and lose access until reactivated.`
          : `${email} will lose access to this organization and its workspaces.`
      }
      who={target?.m.display_name || email}
      confirmLabel={suspend ? "Suspend member" : "Remove member"}
      query={q}
      candidates={members
        .filter((x) => x.status === "active" && x.user_id !== target?.m.user_id)
        .map((x) => ({ id: x.user_id, label: `${x.display_name} (${x.email})` }))}
      busy={busy}
      error={error}
      onConfirm={onConfirm}
      onClose={onClose}
    />
  );
}

function Members({ orgId, viewer }: { orgId: string; viewer: OrgRole }) {
  const members = useAllMembers(orgId);
  const meId = useMe().data?.user.id;
  const orgName = useOrg(orgId).data?.name;
  const patch = usePatchMember(orgId);
  const remove = useRemoveMember(orgId);
  const [err, setErr] = useState<unknown>(null);
  const [target, setTarget] = useState<{ m: OrgMember; mode: "remove" | "suspend" } | null>(null);
  const [dialogErr, setDialogErr] = useState<unknown>(null);
  const [promote, setPromote] = useState<{ m: OrgMember; role: OrgRole } | null>(null);

  if (members.isPending) return <Loading />;
  if (members.isError) return <ErrorState error={members.error} onRetry={() => members.refetch()} />;
  const items = members.data.pages.flatMap((p) => p.items);
  const owners = items.filter((m) => m.role === "owner" && m.status === "active").length;
  const busy = patch.isPending || remove.isPending;
  const onError = (e: unknown) => setErr(e);

  return (
    <div className="space-y-3">
      <ErrorNotice error={err} />
      {items.length === 0 ? (
        <EmptyState title="No members" />
      ) : (
        <DataTable caption="People" stackBelow={900}>
            <thead>
              <tr>
                <Th>Person</Th>
                <Th>Role</Th>
                <Th>Status</Th>
                <Th>Joined</Th>
                <Th>
                  <span className="sr-only">Actions</span>
                </Th>
              </tr>
            </thead>
            <tbody>
              {items.map((m) => (
                <MemberRow
                  key={m.user_id}
                  m={m}
                  viewer={viewer}
                  busy={busy}
                  mine={m.user_id === meId}
                  lastOwner={m.role === "owner" && owners === 1}
                  orgName={orgName}
                  onRole={(role) => {
                    setErr(null);
                    // Second rung of the confirm ladder: a role that widens what the person can do asks first.
                    if (ROLE_RANK[role] > ROLE_RANK[m.role] && role !== "member") setPromote({ m, role });
                    else patch.mutate({ user: m.user_id, body: { role } }, { onError });
                  }}
                  onStatus={(status) => {
                    setErr(null);
                    // RFE-021: suspending shows what it would disable first.
                    if (status === "suspended") setTarget({ m, mode: "suspend" });
                    else patch.mutate({ user: m.user_id, body: { status } }, { onError });
                  }}
                  onRemove={() => setTarget({ m, mode: "remove" })}
                />
              ))}
            </tbody>
        </DataTable>
      )}
      <LoadMore query={members} />
      <ConfirmDialog
        open={promote !== null}
        onOpenChange={(o) => {
          if (!o) setPromote(null);
        }}
        title={promote ? `Make ${promote.m.display_name || promote.m.email} ${promote.role === "owner" ? "an owner" : "an admin"}?` : "Change role"}
        description={promote ? (PROMOTE_SENTENCE[promote.role] ?? "") : ""}
        confirmLabel={promote ? (promote.role === "owner" ? "Promote to Owner" : "Make admin") : "Change role"}
        busy={patch.isPending}
        onConfirm={() => {
          if (!promote) return;
          const { m, role } = promote;
          setPromote(null);
          patch.mutate({ user: m.user_id, body: { role } }, { onError });
        }}
      />
      <OffboardDialog
        orgId={orgId}
        target={target}
        members={items}
        busy={busy}
        error={dialogErr}
        onClose={() => {
          setTarget(null);
          setDialogErr(null);
        }}
        onConfirm={(reassign_to) => {
          if (!target) return;
          setDialogErr(null);
          const done = { onSuccess: () => setTarget(null), onError: (e: unknown) => setDialogErr(e) };
          if (target.mode === "remove") {
            remove.mutate({ user: target.m.user_id, reassign_to }, done);
          } else {
            patch.mutate({ user: target.m.user_id, body: { status: "suspended", reassign_to } }, done);
          }
        }}
      />
    </div>
  );
}

function Invitations({ orgId }: { orgId: string }) {
  const invites = useInvitations(orgId);
  const cancel = useCancelInvitation(orgId);
  if (invites.isPending) return <Loading />;
  if (invites.isError) return <ErrorState error={invites.error} onRetry={() => invites.refetch()} />;
  const items = invites.data.items;
  if (items.length === 0) return <EmptyState title="No pending invitations" />;
  return (
    <div className="space-y-3">
      <ErrorNotice error={cancel.error} />
      <DataTable caption="Pending invitations" stackBelow={900}>
        <thead>
          <tr>
            <Th>Email</Th>
            <Th>Role</Th>
            <Th>Workspaces</Th>
            <Th>Expires</Th>
            <Th>
              <span className="sr-only">Actions</span>
            </Th>
          </tr>
        </thead>
        <tbody>
          {items.map((i) => (
            <tr key={i.id}>
              <Td cell="title" className="break-all font-medium">
                {i.email}
              </Td>
              <Td label="Role">{cap(i.role)}</Td>
              <Td label="Workspaces">{i.workspace_ids.length}</Td>
              <Td label="Expires" className="whitespace-nowrap text-ink-2">
                <When value={i.expires_at} />
              </Td>
              <Td cell="actions" className="text-right">
                <Button
                  variant="outline"
                  size="sm"
                  disabled={cancel.isPending}
                  aria-label={`Cancel invitation for ${i.email}`}
                  onClick={() => cancel.mutate(i.id)}
                >
                  Cancel
                </Button>
              </Td>
            </tr>
          ))}
        </tbody>
      </DataTable>
    </div>
  );
}

export function MembersView({ orgId, viewer }: { orgId: string; viewer: OrgRole }) {
  const instance = useInstance();
  return (
    <div className="max-w-page space-y-8">
      <PageHeader
        title="Members"
        description="People in this organization and the invitations still waiting."
        actions={<InviteDialog orgId={orgId} />}
        className="mb-6"
      />
      <Section id="members" title="People">
        <Members orgId={orgId} viewer={viewer} />
      </Section>
      <Section id="invites" title="Pending invitations">
        <div className="space-y-3">
          {instance.data?.email_delivery === "log" ? (
            <Notice role="status">Email is not configured: invitation links are written to the server log.</Notice>
          ) : null}
          <Invitations orgId={orgId} />
        </div>
      </Section>
    </div>
  );
}
