"use client";

import { useState } from "react";

import { fmtDate } from "@/components/account/key-table";
import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { OffboardingDialog } from "@/components/automations/offboarding-dialog";
import { IdpRoleBadge } from "@/components/identity/idp-role-badge";
import { DataTable, Td, Th } from "@/components/ui/data-table";
import { inputClass } from "@/components/ui/field";
import { Notice } from "@/components/ui/notice";
import { PageHeader } from "@/components/ui/page-header";
import { Section } from "@/components/ui/section";
import { cn } from "@/lib/utils";
import { errorMessage } from "@/lib/api/errors";
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
import { useInstance } from "@/lib/api/queries";

import { InviteDialog } from "./invite-dialog";

export function MemberRow({
  m,
  viewer,
  busy,
  onRole,
  onStatus,
  onRemove,
}: {
  m: OrgMember;
  viewer: OrgRole;
  busy?: boolean;
  onRole: (role: OrgRole) => void;
  onStatus: (status: "active" | "suspended") => void;
  onRemove: () => void;
}) {
  const fromIdp = m.idp_value ?? null;
  // A role the IdP gives is read-only here (409 managed_by_idp on the API).
  const options = fromIdp ? [] : roleOptions(viewer, m.role);
  const manage = canManageMember(viewer, m.role);
  return (
    <tr>
      <Td>
        <div className="font-medium">{m.display_name}</div>
        <div className="text-muted">{m.email}</div>
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
                  {r}
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
            {m.role}
            {fromIdp ? <IdpRoleBadge value={fromIdp} /> : null}
          </span>
        )}
      </Td>
      <Td>
        {m.status === "pending" ? (
          <Badge
            tone="pending"
            title="Your identity provider prepared this membership for an existing account. It becomes active at the person's next sign-in."
            className="normal-case tracking-normal"
          >
            Waiting for first sign-in
          </Badge>
        ) : (
          <Badge tone={m.status === "active" ? "accent" : "pending"}>{m.status}</Badge>
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
      <Td className="whitespace-nowrap text-ink-2">{fmtDate(m.created_at)}</Td>
      <Td>
        {manage ? (
          <div className="flex gap-2">
            {m.status === "pending" ? null : (
              <Button
                variant="outline"
                size="sm"
                disabled={busy}
                aria-label={`${m.status === "active" ? "Suspend" : "Reactivate"} ${m.email}`}
                onClick={() => onStatus(m.status === "active" ? "suspended" : "active")}
              >
                {m.status === "active" ? "Suspend" : "Reactivate"}
              </Button>
            )}
            {fromIdp ? null : (
              <Button
                variant="danger"
                size="sm"
                disabled={busy}
                aria-label={`Remove ${m.email}`}
                onClick={onRemove}
              >
                Remove
              </Button>
            )}
          </div>
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
  const patch = usePatchMember(orgId);
  const remove = useRemoveMember(orgId);
  const [err, setErr] = useState<unknown>(null);
  const [target, setTarget] = useState<{ m: OrgMember; mode: "remove" | "suspend" } | null>(null);
  const [dialogErr, setDialogErr] = useState<unknown>(null);

  if (members.isPending) return <Loading />;
  if (members.isError) return <ErrorState error={members.error} onRetry={() => members.refetch()} />;
  const items = members.data.pages.flatMap((p) => p.items);
  const busy = patch.isPending || remove.isPending;
  const onError = (e: unknown) => setErr(e);

  return (
    <div className="space-y-3">
      {err ? (
        <Notice tone="danger" role="alert">
          {errorMessage(err)}
        </Notice>
      ) : null}
      {items.length === 0 ? (
        <EmptyState title="No members" />
      ) : (
        <DataTable caption="People">
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
                  onRole={(role) => {
                    setErr(null);
                    patch.mutate({ user: m.user_id, body: { role } }, { onError });
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
      {members.hasNextPage ? (
        <Button
          variant="outline"
          disabled={members.isFetchingNextPage}
          onClick={() => void members.fetchNextPage()}
        >
          Load more
        </Button>
      ) : null}
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
      {cancel.error ? (
        <Notice tone="danger" role="alert">
          {errorMessage(cancel.error)}
        </Notice>
      ) : null}
      <DataTable caption="Pending invitations">
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
              <Td>{i.email}</Td>
              <Td>{i.role}</Td>
              <Td>{i.workspace_ids.length}</Td>
              <Td className="whitespace-nowrap text-ink-2">{fmtDate(i.expires_at)}</Td>
              <Td className="text-right">
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
    <div className="max-w-[1160px] space-y-8">
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
