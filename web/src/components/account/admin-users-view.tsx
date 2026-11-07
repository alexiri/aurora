"use client";

import { useState } from "react";

import { ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import { inputClass } from "@/components/ui/field";
import { useAdminUsers, usePatchAdminUser, type AdminUser } from "@/lib/api/account";
import { errorMessage } from "@/lib/api/errors";
import { useSession } from "@/lib/api/queries";

import { fmtDate } from "./key-table";

export function NotFoundPage() {
  return (
    <div className="space-y-2">
      <h1 className="text-xl font-semibold">404</h1>
      <p className="text-sm text-neutral-600">This page could not be found.</p>
    </div>
  );
}

export function UsersTable({
  users,
  selfId,
}: {
  users: AdminUser[];
  selfId?: string;
}) {
  const patch = usePatchAdminUser();
  const [err, setErr] = useState<{ id: string; error: unknown } | null>(null);

  function change(u: AdminUser, body: { is_instance_admin?: boolean; disabled?: boolean }) {
    setErr(null);
    patch.mutate({ id: u.id, body }, { onError: (error) => setErr({ id: u.id, error }) });
  }

  if (users.length === 0) return <p className="text-sm text-neutral-600">No users found.</p>;
  return (
    <div className="overflow-x-auto">
      <table className="w-full text-left text-sm">
        <thead className="text-neutral-500">
          <tr>
            <th className="py-2 pr-4 font-medium">User</th>
            <th className="py-2 pr-4 font-medium">Last login</th>
            <th className="py-2 pr-4 font-medium">Status</th>
            <th className="py-2 font-medium">Actions</th>
          </tr>
        </thead>
        <tbody>
          {users.map((u) => {
            const disabled = !!u.disabled_at;
            const isSelf = u.id === selfId;
            return (
              <tr key={u.id} className="border-t border-neutral-200 align-top">
                <td className="py-2 pr-4">
                  <div className="font-medium">{u.display_name}</div>
                  <div className="text-neutral-500">{u.email}</div>
                </td>
                <td className="py-2 pr-4">{fmtDate(u.last_login_at)}</td>
                <td className="py-2 pr-4">
                  {u.is_instance_admin ? "Instance admin" : "User"}
                  {disabled ? ", disabled" : ""}
                </td>
                <td className="space-y-1 py-2">
                  <div className="flex gap-2">
                    <Button
                      variant="outline"
                      size="sm"
                      disabled={patch.isPending}
                      onClick={() => change(u, { is_instance_admin: !u.is_instance_admin })}
                      aria-label={`${u.is_instance_admin ? "Remove admin from" : "Make admin"} ${u.email}`}
                    >
                      {u.is_instance_admin ? "Remove admin" : "Make admin"}
                    </Button>
                    <Button
                      variant="outline"
                      size="sm"
                      disabled={patch.isPending}
                      onClick={() => change(u, { disabled: !disabled })}
                      aria-label={`${disabled ? "Enable" : "Disable"} ${u.email}`}
                    >
                      {disabled ? "Enable" : "Disable"}
                    </Button>
                  </div>
                  {isSelf ? <p className="text-xs text-neutral-500">This is you.</p> : null}
                  {err?.id === u.id ? (
                    <p role="alert" className="text-xs text-red-700">
                      {errorMessage(err.error)}
                    </p>
                  ) : null}
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}

export function AdminUsersView() {
  const session = useSession();
  const [input, setInput] = useState("");
  const [q, setQ] = useState("");
  const isAdmin = session.data?.user.is_instance_admin === true;
  const users = useAdminUsers(q, isAdmin);

  if (session.isPending) return <Loading />;
  if (session.isError) return <ErrorState error={session.error} onRetry={() => session.refetch()} />;
  if (!isAdmin) return <NotFoundPage />;

  return (
    <div className="space-y-4">
      <h1 className="text-xl font-semibold">Users</h1>
      <form
        role="search"
        className="flex max-w-md gap-2"
        onSubmit={(e) => {
          e.preventDefault();
          setQ(input.trim());
        }}
      >
        <input
          aria-label="Search users"
          placeholder="Search by name or email"
          className={inputClass}
          value={input}
          onChange={(e) => setInput(e.target.value)}
        />
        <Button type="submit" variant="outline">
          Search
        </Button>
      </form>
      {users.isPending && users.fetchStatus !== "idle" ? <Loading /> : null}
      {users.isError ? <ErrorState error={users.error} onRetry={() => users.refetch()} /> : null}
      {users.data ? (
        <UsersTable
          users={users.data.pages.flatMap((p) => p.items)}
          selfId={session.data.user.id}
        />
      ) : null}
      {users.hasNextPage ? (
        <Button variant="outline" disabled={users.isFetchingNextPage} onClick={() => void users.fetchNextPage()}>
          Load more
        </Button>
      ) : null}
    </div>
  );
}
