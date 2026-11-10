"use client";

import { useState } from "react";

import { IdpRoleBadge } from "@/components/identity/idp-role-badge";
import { IdpRolesList } from "@/components/identity/idp-roles-list";
import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { DataTable, Td, Th } from "@/components/ui/data-table";
import { inputClass } from "@/components/ui/field";
import { Notice } from "@/components/ui/notice";
import { PageHeader } from "@/components/ui/page-header";
import { useAdminUsers, usePatchAdminUser, type AdminUser } from "@/lib/api/account";
import { errorMessage } from "@/lib/api/errors";
import { useSession } from "@/lib/api/queries";

import { fmtDate } from "./key-table";

export function NotFoundPage() {
  return (
    <div className="max-w-[880px]">
      <PageHeader title="404" description="This page could not be found." />
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

  if (users.length === 0) return <EmptyState title="No users found" />;
  return (
    <DataTable caption="Users">
      <thead>
        <tr>
          <Th>User</Th>
          <Th>Last login</Th>
          <Th>Status</Th>
          <Th>Actions</Th>
        </tr>
      </thead>
      <tbody>
        {users.map((u) => {
          const disabled = !!u.disabled_at;
          const isSelf = u.id === selfId;
          return (
            <tr key={u.id}>
              <Td>
                <div className="font-medium">{u.display_name}</div>
                <div className="text-muted">{u.email}</div>
              </Td>
              <Td className="whitespace-nowrap text-ink-2">{fmtDate(u.last_login_at)}</Td>
              <Td>
                <div className="flex flex-wrap items-center gap-1.5">
                  <Badge tone={u.is_instance_admin ? "accent" : "neutral"}>
                    {u.is_instance_admin ? "Instance admin" : "User"}
                  </Badge>
                  {u.instance_admin_idp_value ? <IdpRoleBadge value={u.instance_admin_idp_value} /> : null}
                  {disabled ? <Badge tone="danger">disabled</Badge> : null}
                </div>
                {u.idp_roles ? (
                  <details className="mt-1.5 text-xs">
                    <summary className="cursor-pointer text-muted">
                      Role values ({u.idp_roles.values.length})
                    </summary>
                    <div className="mt-1.5 min-w-60">
                      <IdpRolesList roles={u.idp_roles} />
                    </div>
                  </details>
                ) : null}
              </Td>
              <Td className="space-y-1">
                <div className="flex gap-2">
                  <Button
                    variant="outline"
                    size="sm"
                    disabled={patch.isPending || !!u.instance_admin_idp_value}
                    title={u.instance_admin_idp_value ? "Granted by an identity provider role" : undefined}
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
                {isSelf ? <p className="text-xs text-muted">This is you.</p> : null}
                {err?.id === u.id ? (
                  <Notice tone="danger" role="alert" className="max-w-xs p-2 text-xs">
                    {errorMessage(err.error)}
                  </Notice>
                ) : null}
              </Td>
            </tr>
          );
        })}
      </tbody>
    </DataTable>
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
    <div className="max-w-[1160px] space-y-4">
      <PageHeader title="Users" description="Everyone on this instance. Admins can promote or disable accounts." className="mb-2" />
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
