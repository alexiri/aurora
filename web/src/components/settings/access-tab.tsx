"use client";

import { useState } from "react";

import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import { inputClass } from "@/components/ui/field";
import {
  useAccess,
  useDeleteAccess,
  useOrgMembers,
  usePutAccess,
  type AccessBinding,
} from "@/lib/api/account";
import { errorMessage } from "@/lib/api/errors";

type Role = "owner" | "member";

function AddBinding({
  wsId,
  orgId,
  bound,
}: {
  wsId: string;
  orgId: string;
  bound: Set<string>;
}) {
  const members = useOrgMembers(orgId);
  const put = usePutAccess(wsId);
  const [user, setUser] = useState("");
  const [role, setRole] = useState<Role>("member");

  // Plain org members cannot list members (403): hide the picker entirely.
  if (members.isPending || members.isError) return null;
  const candidates = members.data.items.filter((m) => !bound.has(m.user_id));
  if (candidates.length === 0) {
    return <p className="text-sm text-neutral-600">Every active org member already has access.</p>;
  }
  return (
    <form
      aria-label="Add access"
      className="flex flex-wrap items-end gap-2"
      onSubmit={(e) => {
        e.preventDefault();
        if (!user) return;
        put.mutate({ user, role }, { onSuccess: () => setUser("") });
      }}
    >
      <div>
        <label htmlFor="acc-add-user" className="block text-sm font-medium">
          Organization member
        </label>
        <select
          id="acc-add-user"
          className={inputClass}
          value={user}
          onChange={(e) => setUser(e.target.value)}
        >
          <option value="">Select a member…</option>
          {candidates.map((m) => (
            <option key={m.user_id} value={m.user_id}>
              {m.display_name} ({m.email})
            </option>
          ))}
        </select>
      </div>
      <div>
        <label htmlFor="acc-add-role" className="block text-sm font-medium">
          Role
        </label>
        <select
          id="acc-add-role"
          className={inputClass}
          value={role}
          onChange={(e) => setRole(e.target.value as Role)}
        >
          <option value="member">member</option>
          <option value="owner">owner</option>
        </select>
      </div>
      <Button type="submit" disabled={!user || put.isPending}>
        Add
      </Button>
      {put.error ? (
        <p role="alert" className="basis-full text-sm text-red-700">
          {errorMessage(put.error)}
        </p>
      ) : null}
    </form>
  );
}

function Row({
  b,
  wsId,
  canOwn,
  onError,
}: {
  b: AccessBinding;
  wsId: string;
  canOwn: boolean;
  onError: (e: unknown) => void;
}) {
  const put = usePutAccess(wsId);
  const del = useDeleteAccess(wsId);
  const busy = put.isPending || del.isPending;
  return (
    <tr className="border-t border-neutral-200">
      <td className="py-2 pr-4">
        <div className="font-medium">{b.display_name}</div>
        <div className="text-neutral-500">{b.email}</div>
      </td>
      <td className="py-2 pr-4">
        {b.implicit ? (
          <span>
            owner{" "}
            <span className="rounded bg-neutral-100 px-1.5 py-0.5 text-xs" title="Organization owner or admin">
              Implicit (org)
            </span>
          </span>
        ) : canOwn ? (
          <select
            aria-label={`Role for ${b.email}`}
            className={inputClass}
            value={b.role}
            disabled={busy}
            onChange={(e) =>
              put.mutate({ user: b.user_id, role: e.target.value as Role }, { onError })
            }
          >
            <option value="member">member</option>
            <option value="owner">owner</option>
          </select>
        ) : (
          b.role
        )}
      </td>
      <td className="py-2">
        {canOwn && !b.implicit ? (
          <Button
            variant="outline"
            size="sm"
            disabled={busy}
            aria-label={`Remove ${b.email}`}
            onClick={() => del.mutate(b.user_id, { onError })}
          >
            Remove
          </Button>
        ) : null}
      </td>
    </tr>
  );
}

export function AccessTab({
  wsId,
  orgId,
  canOwn,
}: {
  wsId: string;
  orgId: string;
  canOwn: boolean;
}) {
  const access = useAccess(wsId);
  const [err, setErr] = useState<unknown>(null);
  if (access.isPending) return <Loading />;
  if (access.isError) return <ErrorState error={access.error} onRetry={() => access.refetch()} />;
  const items = access.data.items;
  const bound = new Set(items.map((b) => b.user_id));
  return (
    <div className="space-y-4">
      {!canOwn ? (
        <p className="rounded-md bg-neutral-100 p-3 text-sm text-neutral-700">
          Only workspace owners can change who has access.
        </p>
      ) : null}
      {err ? (
        <p role="alert" className="text-sm text-red-700">
          {errorMessage(err)}
        </p>
      ) : null}
      {items.length === 0 ? (
        <EmptyState title="No one has access yet" />
      ) : (
        <table className="w-full text-left text-sm">
          <thead className="text-neutral-500">
            <tr>
              <th className="py-2 pr-4 font-medium">Person</th>
              <th className="py-2 pr-4 font-medium">Role</th>
              <th className="py-2 font-medium">
                <span className="sr-only">Actions</span>
              </th>
            </tr>
          </thead>
          <tbody>
            {items.map((b) => (
              <Row key={b.user_id} b={b} wsId={wsId} canOwn={canOwn} onError={setErr} />
            ))}
          </tbody>
        </table>
      )}
      {canOwn ? <AddBinding wsId={wsId} orgId={orgId} bound={bound} /> : null}
    </div>
  );
}
