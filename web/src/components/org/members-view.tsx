"use client";

import { useState } from "react";

import { fmtDate } from "@/components/account/key-table";
import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import { inputClass } from "@/components/ui/field";
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
  const options = roleOptions(viewer, m.role);
  const manage = canManageMember(viewer, m.role);
  return (
    <tr className="border-t border-neutral-200">
      <td className="py-2 pr-4">
        <div className="font-medium">{m.display_name}</div>
        <div className="text-neutral-500">{m.email}</div>
      </td>
      <td className="py-2 pr-4">
        {options.length > 0 ? (
          <select
            aria-label={`Role for ${m.email}`}
            className={inputClass}
            value={m.role}
            disabled={busy}
            onChange={(e) => onRole(e.target.value as OrgRole)}
          >
            {options.map((r) => (
              <option key={r} value={r}>
                {r}
              </option>
            ))}
          </select>
        ) : (
          m.role
        )}
      </td>
      <td className="py-2 pr-4">{m.status}</td>
      <td className="py-2 pr-4">{fmtDate(m.created_at)}</td>
      <td className="py-2">
        {manage ? (
          <div className="flex gap-2">
            <Button
              variant="outline"
              size="sm"
              disabled={busy}
              aria-label={`${m.status === "active" ? "Suspend" : "Reactivate"} ${m.email}`}
              onClick={() => onStatus(m.status === "active" ? "suspended" : "active")}
            >
              {m.status === "active" ? "Suspend" : "Reactivate"}
            </Button>
            <Button
              variant="outline"
              size="sm"
              disabled={busy}
              aria-label={`Remove ${m.email}`}
              onClick={onRemove}
            >
              Remove
            </Button>
          </div>
        ) : null}
      </td>
    </tr>
  );
}

function Members({ orgId, viewer }: { orgId: string; viewer: OrgRole }) {
  const members = useAllMembers(orgId);
  const patch = usePatchMember(orgId);
  const remove = useRemoveMember(orgId);
  const [err, setErr] = useState<unknown>(null);
  const [target, setTarget] = useState<OrgMember | null>(null);

  if (members.isPending) return <Loading />;
  if (members.isError) return <ErrorState error={members.error} onRetry={() => members.refetch()} />;
  const items = members.data.pages.flatMap((p) => p.items);
  const busy = patch.isPending || remove.isPending;
  const onError = (e: unknown) => setErr(e);

  return (
    <div className="space-y-2">
      {err ? (
        <p role="alert" className="text-sm text-red-700">
          {errorMessage(err)}
        </p>
      ) : null}
      {items.length === 0 ? (
        <EmptyState title="No members" />
      ) : (
        <div className="overflow-x-auto">
          <table className="w-full text-left text-sm">
            <thead className="text-neutral-500">
              <tr>
                <th className="py-2 pr-4 font-medium">Person</th>
                <th className="py-2 pr-4 font-medium">Role</th>
                <th className="py-2 pr-4 font-medium">Status</th>
                <th className="py-2 pr-4 font-medium">Joined</th>
                <th className="py-2 font-medium">
                  <span className="sr-only">Actions</span>
                </th>
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
                    patch.mutate({ user: m.user_id, body: { status } }, { onError });
                  }}
                  onRemove={() => setTarget(m)}
                />
              ))}
            </tbody>
          </table>
        </div>
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
      <Dialog open={target !== null} onOpenChange={(o) => !o && setTarget(null)}>
        <DialogContent
          title="Remove member?"
          description={`${target?.email ?? ""} will lose access to this organization and its workspaces.`}
        >
          <div className="flex justify-end gap-2">
            <Button variant="outline" onClick={() => setTarget(null)}>
              Cancel
            </Button>
            <Button
              disabled={remove.isPending}
              onClick={() => {
                if (!target) return;
                setErr(null);
                remove.mutate(target.user_id, {
                  onError,
                  onSettled: () => setTarget(null),
                });
              }}
            >
              Remove member
            </Button>
          </div>
        </DialogContent>
      </Dialog>
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
    <div className="space-y-2">
      {cancel.error ? (
        <p role="alert" className="text-sm text-red-700">
          {errorMessage(cancel.error)}
        </p>
      ) : null}
      <table className="w-full text-left text-sm">
        <thead className="text-neutral-500">
          <tr>
            <th className="py-2 pr-4 font-medium">Email</th>
            <th className="py-2 pr-4 font-medium">Role</th>
            <th className="py-2 pr-4 font-medium">Workspaces</th>
            <th className="py-2 pr-4 font-medium">Expires</th>
            <th className="py-2 font-medium">
              <span className="sr-only">Actions</span>
            </th>
          </tr>
        </thead>
        <tbody>
          {items.map((i) => (
            <tr key={i.id} className="border-t border-neutral-200">
              <td className="py-2 pr-4">{i.email}</td>
              <td className="py-2 pr-4">{i.role}</td>
              <td className="py-2 pr-4">{i.workspace_ids.length}</td>
              <td className="py-2 pr-4">{fmtDate(i.expires_at)}</td>
              <td className="py-2">
                <Button
                  variant="outline"
                  size="sm"
                  disabled={cancel.isPending}
                  aria-label={`Cancel invitation for ${i.email}`}
                  onClick={() => cancel.mutate(i.id)}
                >
                  Cancel
                </Button>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

export function MembersView({ orgId, viewer }: { orgId: string; viewer: OrgRole }) {
  const instance = useInstance();
  return (
    <div className="max-w-4xl space-y-6">
      <div className="flex items-center justify-between">
        <h1 className="text-xl font-semibold">Members</h1>
        <InviteDialog orgId={orgId} />
      </div>
      <section aria-labelledby="members-h" className="space-y-2">
        <h2 id="members-h" className="text-sm font-medium text-neutral-600">
          People
        </h2>
        <Members orgId={orgId} viewer={viewer} />
      </section>
      <section aria-labelledby="invites-h" className="space-y-2">
        <h2 id="invites-h" className="text-sm font-medium text-neutral-600">
          Pending invitations
        </h2>
        {instance.data?.email_delivery === "log" ? (
          <p className="rounded-md bg-neutral-100 p-3 text-sm text-neutral-700">
            Email is not configured: invitation links are written to the server log.
          </p>
        ) : null}
        <Invitations orgId={orgId} />
      </section>
    </div>
  );
}
